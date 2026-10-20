use chrono::{Local, Utc};
use photosync_core::{
    capacity::{self, Capacity},
    local::walk_files,
    profile::{
        bootstrap as bootstrap_files, profile_path, read_profile, write_profile, BootstrapState,
        Profile, PROFILE_DIR,
    },
    project::Project,
    release::{
        recover, ReleaseContext, ReleaseJournal, ReleaseRecord, ReleaseRunner, ReleaseTimeouts,
    },
    settings::Settings,
    status_client::StatusClient,
    syncthing::SyncthingClient,
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tauri::{Emitter, Manager};

#[cfg(target_os = "android")]
struct AndroidVault(tauri::plugin::PluginHandle<tauri::Wry>);

#[cfg(target_os = "android")]
fn vault(app: &tauri::AppHandle) -> tauri::State<'_, AndroidVault> {
    app.state::<AndroidVault>()
}

struct AppState {
    settings_path: PathBuf,
    cache_path: PathBuf,
    journal_dir: PathBuf,
}

impl AppState {
    fn new(app: &tauri::AppHandle) -> Result<Self, String> {
        let data_dir = app.path().app_data_dir().map_err(err)?;
        fs::create_dir_all(&data_dir).map_err(err)?;
        let journal_dir = data_dir.join("journals");
        fs::create_dir_all(&journal_dir).map_err(err)?;
        Ok(Self {
            settings_path: data_dir.join("settings.json"),
            cache_path: data_dir.join("projects.json"),
            journal_dir,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    settings: Settings,
    api_key_set: bool,
    status_token_set: bool,
    configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageInfo {
    path: String,
    total_bytes: u64,
    free_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapView {
    created: bool,
    present: bool,
    mismatch: bool,
    existing: Option<String>,
    profile_path: String,
    stignore_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupCheck {
    syncthing_ok: bool,
    syncthing_error: Option<String>,
    folder_ok: bool,
    folder_state: String,
    folder_error: Option<String>,
    status_ok: bool,
    status_error: Option<String>,
    all_files: bool,
    profile_present: bool,
    stignore_present: bool,
    stignore_device: Option<String>,
    base_ignores: usize,
    storage: Option<StorageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectView {
    path: String,
    name: String,
    year: Option<i32>,
    kind: String,
    published: String,
    active: bool,
    hub_size: u64,
    local_bytes: u64,
    files: u64,
    mtime_ns: i64,
    stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectsView {
    projects: Vec<ProjectView>,
    stale: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityView {
    total_bytes: u64,
    free_bytes: u64,
    active_bytes: u64,
    projected_free_bytes: u64,
    headroom_bytes: u64,
    headroom_percent: u8,
    stale: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingProject {
    path: String,
    name: String,
    pending_files: u64,
    pending_bytes: u64,
    hub_missing: u64,
    mismatch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    syncthing_ok: bool,
    syncthing_error: Option<String>,
    folder_state: String,
    folder_error: Option<String>,
    hub_ok: bool,
    hub_error: Option<String>,
    backup: Option<String>,
    pending: Vec<PendingProject>,
    storage: Option<StorageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseHistory {
    latest: Option<ReleaseRecord>,
    records: Vec<ReleaseRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSettingsInput {
    settings: Settings,
    api_key: Option<String>,
    status_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProjects {
    at: chrono::DateTime<Utc>,
    projects: Vec<ProjectView>,
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn load_settings(state: &AppState) -> Settings {
    fs::read_to_string(&state.settings_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
}

fn save_settings_file(state: &AppState, settings: &Settings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(err)?;
    photosync_core::profile::write_atomic(&state.settings_path, &bytes).map_err(err)
}

fn load_cache(state: &AppState) -> Option<CachedProjects> {
    fs::read_to_string(&state.cache_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn save_cache(state: &AppState, projects: &[ProjectView]) -> Result<(), String> {
    let cache = CachedProjects {
        at: Utc::now(),
        projects: projects.to_vec(),
    };
    let bytes = serde_json::to_vec(&cache).map_err(err)?;
    photosync_core::profile::write_atomic(&state.cache_path, &bytes).map_err(err)
}

fn configured(settings: &Settings, api_key: bool, status_token: bool) -> bool {
    !settings.device_name.is_empty()
        && !settings.folder_id.is_empty()
        && !settings.folder_root.is_empty()
        && !settings.status_server_url.is_empty()
        && api_key
        && status_token
}

fn settings_view(app: &tauri::AppHandle, state: &AppState) -> Result<SettingsView, String> {
    let settings = load_settings(state);
    let api_key_set = secret(app, "syncthing_api_key")?.is_some();
    let status_token_set = secret(app, "status_token")?.is_some();
    Ok(SettingsView {
        configured: configured(&settings, api_key_set, status_token_set),
        settings,
        api_key_set,
        status_token_set,
    })
}

#[cfg(target_os = "android")]
fn secret(app: &tauri::AppHandle, key: &str) -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    struct Response {
        value: Option<String>,
    }
    vault(app)
        .0
        .run_mobile_plugin::<Response>("loadSecret", serde_json::json!({ "key": key }))
        .map(|response| response.value)
        .map_err(err)
}

#[cfg(not(target_os = "android"))]
fn secret(_app: &tauri::AppHandle, key: &str) -> Result<Option<String>, String> {
    // Desktop builds use environment variables only. The Android build is the
    // one that stores secrets in Android Keystore; a macOS vault follows.
    let value = match key {
        "syncthing_api_key" => std::env::var("PHOTOSYNC_API_KEY").ok(),
        "status_token" => std::env::var("PHOTOSYNC_STATUS_TOKEN").ok(),
        _ => None,
    };
    Ok(value.filter(|v| !v.is_empty()))
}

#[cfg(target_os = "android")]
fn save_secret(app: &tauri::AppHandle, key: &str, value: &str) -> Result<(), String> {
    vault(app)
        .0
        .run_mobile_plugin::<serde_json::Value>(
            "saveSecret",
            serde_json::json!({ "key": key, "value": value }),
        )
        .map(|_| ())
        .map_err(err)
}

#[cfg(not(target_os = "android"))]
fn save_secret(_app: &tauri::AppHandle, key: &str, _value: &str) -> Result<(), String> {
    Err(format!(
        "desktop secret storage is not implemented; set {} instead",
        if key == "syncthing_api_key" {
            "PHOTOSYNC_API_KEY"
        } else {
            "PHOTOSYNC_STATUS_TOKEN"
        }
    ))
}

#[cfg(target_os = "android")]
fn set_keep_screen(app: &tauri::AppHandle, enabled: bool) {
    let _ = vault(app).0.run_mobile_plugin::<serde_json::Value>(
        "keepScreenOn",
        serde_json::json!({ "enabled": enabled }),
    );
}

#[cfg(target_os = "android")]
fn release_service(app: &tauri::AppHandle, start: bool) {
    let command = if start {
        "startReleaseService"
    } else {
        "stopReleaseService"
    };
    let _ = vault(app)
        .0
        .run_mobile_plugin::<serde_json::Value>(command, serde_json::json!({}));
}

fn syncthing(settings: &Settings, app: &tauri::AppHandle) -> Result<SyncthingClient, String> {
    let key = secret(app, "syncthing_api_key")?.ok_or("Syncthing API key is not set")?;
    SyncthingClient::new(&settings.syncthing_url, &key).map_err(err)
}

fn status_client(settings: &Settings, app: &tauri::AppHandle) -> Result<StatusClient, String> {
    let token = secret(app, "status_token")?.ok_or("status server token is not set")?;
    StatusClient::new(&settings.status_server_url, &token).map_err(err)
}

fn profile_for(settings: &Settings) -> Result<Profile, String> {
    let root = PathBuf::from(&settings.folder_root);
    let path = profile_path(&root, &settings.device_name).map_err(err)?;
    match read_profile(&path, &settings.always_on) {
        Ok(profile) => Ok(profile),
        Err(_) => Profile::new(
            &settings.device_name,
            settings.cached_base_ignores.clone(),
            settings.always_on.clone(),
            Vec::new(),
        )
        .map_err(err),
    }
}

async fn refresh_base_ignores(
    settings: &mut Settings,
    state: &AppState,
    app: &tauri::AppHandle,
) -> Result<Vec<String>, String> {
    let client = status_client(settings, app)?;
    let lines = client.base_ignores().await.map_err(err)?;
    settings.cached_base_ignores = lines.clone();
    let _ = save_settings_file(state, settings);
    Ok(lines)
}

fn project_view(
    project: &Project,
    size: u64,
    local_bytes: u64,
    files: u64,
    mtime_ns: i64,
    active: bool,
    stale: bool,
) -> ProjectView {
    ProjectView {
        path: project.originals.clone(),
        name: project.name.clone(),
        year: project.year,
        kind: match project.kind {
            photosync_core::project::ProjectKind::Year => "year".into(),
            photosync_core::project::ProjectKind::Project => "project".into(),
        },
        published: project.published.clone(),
        active,
        hub_size: size,
        local_bytes,
        files,
        mtime_ns,
        stale,
    }
}

async fn fetch_projects(
    settings: &Settings,
    app: &tauri::AppHandle,
    profile: &Profile,
) -> Result<Vec<ProjectView>, String> {
    let client = status_client(settings, app)?;
    let layout = &settings.layout;
    let years_root = format!("{}/{}", layout.originals, layout.years);
    let projects_root = format!("{}/{}", layout.originals, layout.projects);
    let published_years_root = format!("{}/{}", layout.published, layout.years);
    let published_projects_root = format!("{}/{}", layout.published, layout.projects);

    let years = client.sizes(&years_root).await.map_err(err)?;
    let mut originals = Vec::new();
    for year in &years {
        originals.extend(client.sizes(&year.path).await.map_err(err)?);
    }
    originals.extend(client.sizes(&projects_root).await.map_err(err)?);

    let published_years = client
        .sizes(&published_years_root)
        .await
        .unwrap_or_default();
    let mut published_sizes = Vec::new();
    for year in &published_years {
        published_sizes.extend(client.sizes(&year.path).await.unwrap_or_default());
    }
    published_sizes.extend(
        client
            .sizes(&published_projects_root)
            .await
            .unwrap_or_default(),
    );
    let published: std::collections::HashMap<&str, &photosync_core::status_client::ProjectSize> =
        published_sizes
            .iter()
            .map(|p| (p.path.as_str(), p))
            .collect();

    let ignores = photosync_core::ignore::IgnoreSet::parse(
        settings.cached_base_ignores.iter().map(String::as_str),
    )
    .unwrap_or_default();
    let mut projects = Vec::new();
    for size in &originals {
        let Ok(project) = Project::from_originals_in(&size.path, layout) else {
            continue;
        };
        let twin = published.get(project.published.as_str());
        let active = profile.active.contains(&project.originals);
        let local_bytes = if active {
            [&project.originals, &project.published]
                .into_iter()
                .filter_map(|path| {
                    walk_files(&PathBuf::from(&settings.folder_root), path, &ignores).ok()
                })
                .flatten()
                .map(|file| file.size)
                .sum()
        } else {
            0
        };
        projects.push(project_view(
            &project,
            size.size + twin.map(|t| t.size).unwrap_or(0),
            local_bytes,
            size.files + twin.map(|t| t.files).unwrap_or(0),
            size.mtime_ns.max(twin.map(|t| t.mtime_ns).unwrap_or(0)),
            active,
            false,
        ));
    }
    for active in &profile.active {
        if projects.iter().any(|p| &p.path == active) {
            continue;
        }
        if let Ok(project) = Project::from_originals_in(active, layout) {
            projects.push(project_view(&project, 0, 0, 0, 0, true, false));
        }
    }
    projects.sort_by(|a, b| {
        b.year
            .unwrap_or(0)
            .cmp(&a.year.unwrap_or(0))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(projects)
}

#[tauri::command]
fn settings(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<SettingsView, String> {
    settings_view(&app, &state)
}

#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    input: SaveSettingsInput,
) -> Result<SettingsView, String> {
    input.settings.validate().map_err(err)?;
    save_settings_file(&state, &input.settings)?;
    if let Some(value) = input.api_key {
        if !value.is_empty() {
            save_secret(&app, "syncthing_api_key", &value)?;
        }
    }
    if let Some(value) = input.status_token {
        if !value.is_empty() {
            save_secret(&app, "status_token", &value)?;
        }
    }
    settings_view(&app, &state)
}

#[tauri::command]
async fn setup_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SetupCheck, String> {
    let mut settings = load_settings(&state);
    let mut syncthing_ok = false;
    let mut syncthing_error = None;
    let mut folder_ok = false;
    let mut folder_state = String::new();
    let mut folder_error = None;
    let mut status_ok = false;
    let mut status_error = None;
    let mut base_ignores = 0;

    if !settings.syncthing_url.is_empty() {
        match syncthing(&settings, &app) {
            Ok(client) => {
                syncthing_ok = client.system_status().await.is_ok();
                if !syncthing_ok {
                    syncthing_error = Some("system status failed".into());
                }
                match client.folder_status(&settings.folder_id).await {
                    Ok(status) => {
                        folder_ok = status.error.is_empty() && status.state != "error";
                        folder_state = status.state.clone();
                        if !status.error.is_empty() {
                            folder_error = Some(status.error);
                        }
                    }
                    Err(error) => folder_error = Some(err(error)),
                }
            }
            Err(error) => syncthing_error = Some(error),
        }
    }

    if !settings.status_server_url.is_empty() {
        match refresh_base_ignores(&mut settings, &state, &app).await {
            Ok(lines) => {
                status_ok = true;
                base_ignores = lines.len();
            }
            Err(error) => status_error = Some(error),
        }
    }

    let root = PathBuf::from(&settings.folder_root);
    let profile_present = if !settings.device_name.is_empty() && !settings.folder_root.is_empty() {
        profile_path(&root, &settings.device_name)
            .map(|p| p.is_file())
            .unwrap_or(false)
    } else {
        false
    };
    let stignore = root.join(photosync_core::profile::STIGNORE);
    let stignore_text = fs::read_to_string(&stignore).ok();
    let stignore_device = stignore_text
        .as_deref()
        .and_then(photosync_core::profile::parse_stignore);
    let all_files = all_files_access_inner(&app).unwrap_or(false);
    let storage = storage_info_inner(&app, &settings).ok();

    Ok(SetupCheck {
        syncthing_ok,
        syncthing_error,
        folder_ok,
        folder_state,
        folder_error,
        status_ok,
        status_error,
        all_files,
        profile_present,
        stignore_present: stignore.is_file(),
        stignore_device,
        base_ignores,
        storage,
    })
}

#[tauri::command]
async fn bootstrap_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<BootstrapView, String> {
    let mut settings = load_settings(&state);
    if settings.device_name.is_empty() || settings.folder_root.is_empty() {
        return Err("Set the device profile name and folder root first".into());
    }
    let _ = refresh_base_ignores(&mut settings, &state, &app).await;
    let mut profile = profile_for(&settings)?;
    profile.base_ignores = settings.cached_base_ignores.clone();
    profile.always_on = settings.always_on.clone();
    let root = PathBuf::from(&settings.folder_root);
    let state_file = bootstrap_files(&root, &settings.device_name, &profile).map_err(err)?;
    let profile_path = profile_path(&root, &settings.device_name).map_err(err)?;
    let stignore_path = root.join(photosync_core::profile::STIGNORE);
    let (created, present, mismatch, existing) = match state_file {
        BootstrapState::Created => (true, false, false, None),
        BootstrapState::Present => (false, true, false, None),
        BootstrapState::Mismatch { existing } => (false, false, true, Some(existing)),
    };
    if !mismatch {
        if let Ok(client) = syncthing(&settings, &app) {
            let _ = client
                .scan_and_wait(
                    &settings.folder_id,
                    Some(PROFILE_DIR),
                    Duration::from_secs(120),
                    Duration::from_millis(500),
                )
                .await;
            let _ = client
                .scan_and_wait(
                    &settings.folder_id,
                    None,
                    Duration::from_secs(120),
                    Duration::from_millis(500),
                )
                .await;
        }
    }
    Ok(BootstrapView {
        created,
        present,
        mismatch,
        existing,
        profile_path: profile_path.display().to_string(),
        stignore_path: stignore_path.display().to_string(),
    })
}

#[tauri::command]
async fn projects(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ProjectsView, String> {
    let mut settings = load_settings(&state);
    let profile = profile_for(&settings)?;
    let _ = refresh_base_ignores(&mut settings, &state, &app).await;
    match fetch_projects(&settings, &app, &profile).await {
        Ok(projects) => {
            let _ = save_cache(&state, &projects);
            Ok(ProjectsView {
                projects,
                stale: false,
                error: None,
            })
        }
        Err(error) => {
            let cached = load_cache(&state);
            let mut projects = cached.map(|c| c.projects).unwrap_or_default();
            for project in &mut projects {
                project.active = profile.active.contains(&project.path);
                project.stale = true;
            }
            Ok(ProjectsView {
                projects,
                stale: true,
                error: Some(error),
            })
        }
    }
}

#[tauri::command]
async fn capacity(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<CapacityView, String> {
    let mut settings = load_settings(&state);
    let profile = profile_for(&settings)?;
    let storage = storage_info_inner(&app, &settings)?;
    let cached = load_cache(&state);
    let mut stale = false;
    let mut error = None;
    let mut sizes: std::collections::HashMap<String, u64> = cached
        .as_ref()
        .map(|c| {
            c.projects
                .iter()
                .map(|p| (p.path.clone(), p.hub_size))
                .collect()
        })
        .unwrap_or_default();
    match status_client(&settings, &app) {
        Ok(client) => {
            for path in &profile.active {
                if !path.starts_with(&format!("{}/", settings.layout.originals)) {
                    continue;
                }
                if let Ok((size, _)) = size_for(&client, path).await {
                    let mut total = size;
                    if let Ok(project) = Project::from_originals_in(path, &settings.layout) {
                        if let Ok((published, _)) = size_for(&client, &project.published).await {
                            total = total.saturating_add(published);
                        }
                    }
                    sizes.insert(path.clone(), total);
                }
            }
        }
        Err(_) => {
            stale = true;
            error = Some("hub sizes unavailable; using the last known values".into());
        }
    }
    let mut active_bytes = 0u64;
    for path in &profile.active {
        if path.starts_with(&format!("{}/", settings.layout.originals)) {
            active_bytes = active_bytes.saturating_add(sizes.get(path).copied().unwrap_or(0));
        }
    }
    let total = storage.total_bytes;
    let free = storage.free_bytes;
    let headroom_bytes = (u128::from(total) * u128::from(settings.headroom_percent) / 100) as u64;
    let projected = free.saturating_sub(active_bytes);
    let _ = refresh_base_ignores(&mut settings, &state, &app).await;
    Ok(CapacityView {
        total_bytes: total,
        free_bytes: free,
        active_bytes,
        projected_free_bytes: projected,
        headroom_bytes,
        headroom_percent: settings.headroom_percent,
        stale,
        error,
    })
}

async fn size_for(client: &StatusClient, path: &str) -> Result<(u64, u64), String> {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let sizes = client.sizes(parent).await.map_err(err)?;
    Ok(sizes
        .into_iter()
        .find(|size| size.path == path)
        .map(|size| (size.size, size.files))
        .unwrap_or((0, 0)))
}

#[tauri::command]
async fn activate(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ProjectView, String> {
    let mut settings = load_settings(&state);
    let project = Project::from_originals_in(&path, &settings.layout).map_err(err)?;
    let mut profile = profile_for(&settings)?;
    if profile.active.contains(&project.originals) {
        return Ok(project_view(&project, 0, 0, 0, 0, true, false));
    }
    let _ = refresh_base_ignores(&mut settings, &state, &app).await;
    let storage = storage_info_inner(&app, &settings)?;
    let mut originals_size = 0u64;
    let mut files = 0u64;
    let mut size_known = false;
    if let Ok(client) = status_client(&settings, &app) {
        if let Ok((size, count)) = size_for(&client, &project.originals).await {
            originals_size = size;
            files = count;
            size_known = true;
        }
        if let Ok((size, count)) = size_for(&client, &project.published).await {
            originals_size = originals_size.saturating_add(size);
            files = files.saturating_add(count);
        }
    } else if let Some(cache) = load_cache(&state) {
        if let Some(cached) = cache.projects.iter().find(|p| p.path == project.originals) {
            originals_size = cached.hub_size;
            files = cached.files;
            size_known = true;
        }
    }
    if !size_known {
        return Err(
            "Cannot verify the hub size while the status server is unreachable; refusing to activate."
                .into(),
        );
    }
    let projection = capacity::project(
        Capacity {
            total_bytes: storage.total_bytes,
            free_bytes: storage.free_bytes,
        },
        originals_size,
        settings.headroom_percent,
    )
    .map_err(err)?;
    if !projection.allowed {
        return Err(format!(
            "Refusing to activate: {} would leave {} free, below the {}% headroom ({}).",
            bytes(projection.additional_bytes),
            bytes(projection.free_after_bytes),
            projection.headroom_percent,
            bytes(projection.headroom_bytes)
        ));
    }

    let before = profile.clone();
    profile.base_ignores = settings.cached_base_ignores.clone();
    profile.always_on = settings.always_on.clone();
    profile.add_active(&project.originals).map_err(err)?;
    profile.add_active(&project.published).map_err(err)?;
    write_profile(
        &profile_path(&PathBuf::from(&settings.folder_root), &settings.device_name).map_err(err)?,
        &profile,
    )
    .map_err(err)?;
    if let Ok(client) = syncthing(&settings, &app) {
        let result = async {
            client
                .scan_and_wait(
                    &settings.folder_id,
                    Some(PROFILE_DIR),
                    Duration::from_secs(120),
                    Duration::from_millis(500),
                )
                .await?;
            client
                .scan_and_wait(
                    &settings.folder_id,
                    None,
                    Duration::from_secs(120),
                    Duration::from_millis(500),
                )
                .await
        }
        .await;
        if let Err(error) = result {
            let _ = write_profile(
                &profile_path(&PathBuf::from(&settings.folder_root), &settings.device_name)
                    .map_err(err)?,
                &before,
            );
            return Err(format!("Could not reload the profile: {error}"));
        }
    }
    Ok(project_view(
        &project,
        originals_size,
        0,
        files,
        0,
        true,
        false,
    ))
}

#[tauri::command]
async fn new_project(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<ProjectView, String> {
    let settings = load_settings(&state);
    if settings.device_name.is_empty() || settings.folder_root.is_empty() {
        return Err("Set up the device profile and folder root first".into());
    }
    let project = Project::new_year_project_in(Local::now().date_naive(), &name, &settings.layout)
        .map_err(err)?;
    let root = PathBuf::from(&settings.folder_root);
    fs::create_dir_all(photosync_core::local::safe_join(&root, &project.originals).map_err(err)?)
        .map_err(err)?;
    let mut profile = profile_for(&settings)?;
    profile.base_ignores = settings.cached_base_ignores.clone();
    profile.always_on = settings.always_on.clone();
    profile.add_active(&project.originals).map_err(err)?;
    profile.add_active(&project.published).map_err(err)?;
    write_profile(
        &profile_path(&root, &settings.device_name).map_err(err)?,
        &profile,
    )
    .map_err(err)?;
    if let Ok(client) = syncthing(&settings, &app) {
        let _ = client
            .scan_and_wait(
                &settings.folder_id,
                Some(PROFILE_DIR),
                Duration::from_secs(120),
                Duration::from_millis(500),
            )
            .await;
        let _ = client
            .scan_and_wait(
                &settings.folder_id,
                None,
                Duration::from_secs(120),
                Duration::from_millis(500),
            )
            .await;
    }
    Ok(project_view(&project, 0, 0, 0, 0, true, false))
}

fn release_context(
    settings: &Settings,
    app: &tauri::AppHandle,
    dry_run: bool,
) -> Result<ReleaseContext, String> {
    let syncthing = Arc::new(syncthing(settings, app)?);
    let hub = Arc::new(status_client(settings, app)?);
    let root = PathBuf::from(&settings.folder_root);
    Ok(ReleaseContext {
        folder_id: settings.folder_id.clone(),
        folder_root: root.clone(),
        profile_path: profile_path(&root, &settings.device_name).map_err(err)?,
        device: settings.device_name.clone(),
        base_ignores: settings.cached_base_ignores.clone(),
        always_on: settings.always_on.clone(),
        syncthing,
        hub,
        timeouts: ReleaseTimeouts {
            idle: Duration::from_secs(180),
            ignores: Duration::from_secs(60),
            poll: Duration::from_millis(500),
        },
        dry_run,
    })
}

#[tauri::command]
async fn release(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<ReleaseRecord, String> {
    let mut settings = load_settings(&state);
    refresh_base_ignores(&mut settings, &state, &app)
        .await
        .map_err(|error| format!("Cannot release without the hub status server: {error}"))?;
    let project = Project::from_originals_in(&path, &settings.layout).map_err(err)?;
    let ctx = release_context(&settings, &app, false)?;
    let runner =
        ReleaseRunner::new(ctx, ReleaseJournal::new(&state.journal_dir), project).map_err(err)?;

    #[cfg(target_os = "android")]
    {
        set_keep_screen(&app, true);
        release_service(&app, true);
    }
    let handle = app.clone();
    let journal_dir = state.journal_dir.clone();
    let mut task = tauri::async_runtime::spawn(async move { runner.run().await });
    let result = loop {
        tokio::select! {
            record = &mut task => break record,
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if let Ok(Some(record)) = ReleaseJournal::new(&journal_dir).latest() {
                    let _ = handle.emit("release-progress", &record);
                }
            }
        }
    }
    .map_err(|error| format!("release task failed: {error}"))?;
    #[cfg(target_os = "android")]
    {
        release_service(&app, false);
        set_keep_screen(&app, false);
    }
    let _ = app.emit("release-progress", &result);
    Ok(result)
}

#[tauri::command]
async fn recover_release(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ReleaseRecord>, String> {
    let journal = ReleaseJournal::new(&state.journal_dir);
    let Some(latest) = journal.latest().map_err(err)? else {
        return Ok(None);
    };
    if latest.phase.is_terminal() {
        return Ok(None);
    }
    let mut settings = load_settings(&state);
    refresh_base_ignores(&mut settings, &state, &app)
        .await
        .map_err(|error| format!("Cannot recover without the hub status server: {error}"))?;
    let ctx = release_context(&settings, &app, false)?;
    let record = recover(ctx, ReleaseJournal::new(&state.journal_dir), latest).await;
    let _ = app.emit("release-progress", &record);
    Ok(Some(record))
}

#[tauri::command]
fn release_history(state: tauri::State<AppState>) -> Result<ReleaseHistory, String> {
    let records = ReleaseJournal::new(&state.journal_dir)
        .list()
        .map_err(err)?;
    Ok(ReleaseHistory {
        latest: records.first().cloned(),
        records,
    })
}

#[tauri::command]
async fn status(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<StatusView, String> {
    let mut settings = load_settings(&state);
    let profile = profile_for(&settings)?;
    let _ = refresh_base_ignores(&mut settings, &state, &app).await;
    let mut syncthing_ok = false;
    let mut syncthing_error = None;
    let mut folder_state = String::new();
    let mut folder_error = None;
    let mut pending = Vec::new();
    let mut hub_ok = false;
    let mut hub_error = None;
    let mut backup = None;
    let storage = storage_info_inner(&app, &settings).ok();

    if let Ok(client) = syncthing(&settings, &app) {
        match client.folder_status(&settings.folder_id).await {
            Ok(status) => {
                syncthing_ok = status.error.is_empty();
                folder_state = status.state;
                if !status.error.is_empty() {
                    folder_error = Some(status.error);
                }
            }
            Err(error) => syncthing_error = Some(err(error)),
        }
    } else if let Err(error) = syncthing(&settings, &app) {
        syncthing_error = Some(error);
    }

    if let Ok(client) = status_client(&settings, &app) {
        match client.backup().await {
            Ok(value) => {
                hub_ok = true;
                backup = value.last_success.map(|time| time.to_rfc3339());
            }
            Err(error) => hub_error = Some(err(error)),
        }
        if hub_ok {
            let ignores = photosync_core::ignore::IgnoreSet::parse(
                settings.cached_base_ignores.iter().map(String::as_str),
            )
            .unwrap_or_default();
            for active in &profile.active {
                if !active.starts_with(&format!("{}/", settings.layout.originals)) {
                    continue;
                }
                let Ok(project) = Project::from_originals_in(active, &settings.layout) else {
                    continue;
                };
                let mut pending_files = 0u64;
                let mut pending_bytes = 0u64;
                let mut hub_missing = 0u64;
                let mut mismatch = 0u64;
                for path in [&project.originals, &project.published] {
                    let Ok(local) =
                        walk_files(&PathBuf::from(&settings.folder_root), path, &ignores)
                    else {
                        continue;
                    };
                    let Ok(remote) = client.tree(path).await else {
                        continue;
                    };
                    let remote: std::collections::HashMap<
                        &str,
                        &photosync_core::status_client::HubFile,
                    > = remote.iter().map(|f| (f.path.as_str(), f)).collect();
                    for file in local {
                        match remote.get(file.path.as_str()) {
                            None => {
                                pending_files += 1;
                                pending_bytes = pending_bytes.saturating_add(file.size);
                                hub_missing += 1;
                            }
                            Some(hub) if hub.size != file.size || hub.mtime_ns != file.mtime_ns => {
                                pending_files += 1;
                                pending_bytes = pending_bytes.saturating_add(file.size);
                                mismatch += 1;
                            }
                            _ => {}
                        }
                    }
                }
                if pending_files > 0 {
                    pending.push(PendingProject {
                        path: project.originals.clone(),
                        name: project.name.clone(),
                        pending_files,
                        pending_bytes,
                        hub_missing,
                        mismatch,
                    });
                }
            }
        }
    } else if let Err(error) = status_client(&settings, &app) {
        hub_error = Some(error);
    }

    Ok(StatusView {
        syncthing_ok,
        syncthing_error,
        folder_state,
        folder_error,
        hub_ok,
        hub_error,
        backup,
        pending,
        storage,
    })
}

#[tauri::command]
fn storage_info(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<StorageInfo, String> {
    storage_info_inner(&app, &load_settings(&state))
}

#[tauri::command]
fn all_files_access(app: tauri::AppHandle) -> Result<bool, String> {
    all_files_access_inner(&app)
}

#[tauri::command]
fn request_all_files_access(app: tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(Deserialize)]
        struct Response {
            granted: bool,
        }
        return vault(&app)
            .0
            .run_mobile_plugin::<Response>("requestAllFilesAccess", serde_json::json!({}))
            .map(|response| response.granted)
            .map_err(err);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(true)
    }
}

fn all_files_access_inner(app: &tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(Deserialize)]
        struct Response {
            granted: bool,
        }
        return vault(app)
            .0
            .run_mobile_plugin::<Response>("hasAllFilesAccess", serde_json::json!({}))
            .map(|response| response.granted)
            .map_err(err);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(true)
    }
}

fn storage_info_inner(app: &tauri::AppHandle, settings: &Settings) -> Result<StorageInfo, String> {
    let path = if settings.folder_root.is_empty() {
        app.path()
            .app_data_dir()
            .map_err(err)?
            .display()
            .to_string()
    } else {
        settings.folder_root.clone()
    };
    #[cfg(target_os = "android")]
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Response {
            total_bytes: u64,
            free_bytes: u64,
        }
        let response = vault(app)
            .0
            .run_mobile_plugin::<Response>("storageInfo", serde_json::json!({ "path": path }))
            .map_err(err)?;
        return Ok(StorageInfo {
            path,
            total_bytes: response.total_bytes,
            free_bytes: response.free_bytes,
        });
    }
    #[cfg(all(unix, not(target_os = "android")))]
    {
        use std::ffi::CString;
        let c_path = CString::new(path.clone()).map_err(err)?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if result != 0 {
            return Err(format!("statvfs failed for {path}"));
        }
        let block = stat.f_frsize as u64;
        Ok(StorageInfo {
            path,
            total_bytes: stat.f_blocks.saturating_mul(block),
            free_bytes: stat.f_bavail.saturating_mul(block),
        })
    }
    #[cfg(not(unix))]
    {
        Err("storage information is not implemented on this platform".into())
    }
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value as u64, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    builder
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("vault")
                .setup(|app, api| {
                    #[cfg(target_os = "android")]
                    app.manage(AndroidVault(
                        api.register_android_plugin("app.photosync", "PhotosyncPlugin")?,
                    ));
                    #[cfg(not(target_os = "android"))]
                    let _ = (app, api);
                    Ok(())
                })
                .build(),
        )
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings,
            save_settings,
            setup_check,
            bootstrap_folder,
            projects,
            capacity,
            activate,
            new_project,
            release,
            recover_release,
            release_history,
            status,
            storage_info,
            all_files_access,
            request_all_files_access,
        ])
        .run(tauri::generate_context!())
        .expect("error while running photosync");
}
