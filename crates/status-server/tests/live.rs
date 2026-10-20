//! Live tests against two real Syncthing 2.x instances on loopback plus the
//! real status server. Run inside `nix develop` or with `SYNCTHING_BIN` set:
//!
//!   cargo test -p photosync-status --test live -- --test-threads=1

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use photosync_core::{
    profile::{bootstrap, profile_path, read_profile, write_profile, Profile},
    project::Project,
    release::{
        ReleaseContext, ReleaseJournal, ReleaseRunner, ReleaseTimeouts, StepStatus, SyncthingApi,
    },
    status_client::StatusClient,
    syncthing::{FolderStatus, Ignores, SyncthingClient},
    Result as CoreResult,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Write,
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::time::sleep;

const FOLDER: &str = "photos";
const DEVICE: &str = "edge";
const ALWAYS_ON: &str = "inbox/PixelMob";

fn syncthing_bin() -> String {
    std::env::var("SYNCTHING_BIN").unwrap_or_else(|_| "syncthing".into())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Syncthing {
    child: Child,
    api: SyncthingClient,
    id: String,
    gui: u16,
    listen: u16,
}

impl Drop for Syncthing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Syncthing {
    async fn start(home: &Path, name: &str) -> Result<Self> {
        fs::create_dir_all(home)?;
        let status = Command::new(syncthing_bin())
            .args(["generate", "--no-port-probing"])
            .arg(format!("--home={}", home.display()))
            .status()
            .context("running syncthing generate")?;
        if !status.success() {
            bail!("syncthing generate failed for {name}");
        }
        let gui = free_port();
        let listen = free_port();
        let api_key = format!("photosync-live-{name}-0123456789abcdef");
        let log = home.join("serve.log");
        let out = File::create(&log)?;
        let err = out.try_clone()?;
        let child = Command::new(syncthing_bin())
            .arg("serve")
            .arg(format!("--home={}", home.display()))
            .arg(format!("--gui-address=http://127.0.0.1:{gui}"))
            .arg(format!("--gui-apikey={api_key}"))
            .args([
                "--no-browser",
                "--no-restart",
                "--no-upgrade",
                "--no-port-probing",
            ])
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .with_context(|| format!("starting syncthing {name}"))?;
        let api = SyncthingClient::new(&format!("http://127.0.0.1:{gui}"), &api_key)?;
        wait_api(&api, &log).await?;
        let id = api.system_status().await?.my_id;
        Ok(Self {
            child,
            api,
            id,
            gui,
            listen,
        })
    }
}

async fn wait_api(api: &SyncthingClient, log: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match api.system_status().await {
            Ok(_) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    let tail = fs::read_to_string(log).unwrap_or_default();
                    bail!(
                        "Syncthing did not start: {error}\n--- {}\n{}",
                        log.display(),
                        tail.chars()
                            .rev()
                            .take(4000)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>()
                    );
                }
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn device_json(id: &str, name: &str, listen: u16) -> Value {
    json!({
        "deviceID": id,
        "name": name,
        "addresses": [format!("tcp://127.0.0.1:{listen}")],
        "compression": "metadata",
        "introducedBy": "",
        "paused": false,
        "skipIntroductionRemovals": false,
        "allowedNetworks": [],
        "ignoredFolders": [],
        "maxSendKbps": 0,
        "maxRecvKbps": 0,
        "maxRequestKiB": 0,
        "numConnections": 0,
        "remoteGUIPort": 0
    })
}

fn folder_json(path: &Path, remote: &str) -> Value {
    json!({
        "id": FOLDER,
        "label": "photos",
        "filesystemType": "basic",
        "path": path,
        "type": "sendreceive",
        "devices": [{"deviceID": remote, "introducedBy": "", "encryptionPassword": ""}],
        "rescanIntervalS": 3600,
        "fsWatcherEnabled": false,
        "fsWatcherDelayS": 10,
        "ignorePerms": true,
        "autoNormalize": true,
        "minDiskFree": {"value": 1, "unit": "%"},
        "versioning": {"type": "", "params": {}, "cleanupIntervalS": 3600},
        "copiers": 0,
        "pullerMaxPendingKiB": 0,
        "hashers": 0,
        "order": "random",
        "ignoreDelete": false,
        "scanProgressIntervalS": 0,
        "pullerPauseS": 0,
        "maxConflicts": 10,
        "modTimeWindowS": 0,
        "maxConcurrentWrites": 16,
        "disableFsync": false,
        "blockPullOrder": "standard",
        "copyRangeMethod": "standard",
        "caseSensitiveFS": false,
        "junctionsAsDirs": false,
        "markerName": ".stfolder",
        "copyOwnershipFromParent": false
    })
}

async fn configure_options(api: &SyncthingClient, listen: u16) -> Result<()> {
    let mut config = api.config().await?;
    let options = config
        .get_mut("options")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("config has no options"))?;
    options.insert(
        "listenAddresses".into(),
        json!([format!("tcp://127.0.0.1:{listen}")]),
    );
    for key in [
        "globalAnnounceEnabled",
        "localAnnounceEnabled",
        "relaysEnabled",
        "natEnabled",
        "startBrowser",
        "crashReportingEnabled",
    ] {
        options.insert(key.into(), json!(false));
    }
    options.insert("urAccepted".into(), json!(-1));
    options.insert("autoUpgradeIntervalH".into(), json!(0));
    options.insert("unackedNotificationIDs".into(), json!([]));
    config["devices"] = json!([]);
    config["folders"] = json!([]);
    api.set_config(&config).await?;
    Ok(())
}

async fn configure_instance(
    api: &SyncthingClient,
    library: &Path,
    remote_id: &str,
    remote_name: &str,
    remote_listen: u16,
    listen: u16,
    with_folder: bool,
) -> Result<()> {
    let mut config = api.config().await?;
    let options = config
        .get_mut("options")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("config has no options"))?;
    options.insert(
        "listenAddresses".into(),
        json!([format!("tcp://127.0.0.1:{listen}")]),
    );
    for key in [
        "globalAnnounceEnabled",
        "localAnnounceEnabled",
        "relaysEnabled",
        "natEnabled",
        "startBrowser",
        "crashReportingEnabled",
    ] {
        options.insert(key.into(), json!(false));
    }
    options.insert("urAccepted".into(), json!(-1));
    options.insert("autoUpgradeIntervalH".into(), json!(0));
    options.insert("unackedNotificationIDs".into(), json!([]));
    config["devices"] = json!([device_json(remote_id, remote_name, remote_listen)]);
    config["folders"] = if with_folder {
        json!([folder_json(library, remote_id)])
    } else {
        json!([])
    };
    api.set_config(&config).await?;
    Ok(())
}

async fn wait_connected(api: &SyncthingClient, remote: &str, what: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let connections = api.connections().await?;
        if connections
            .connections
            .get(remote)
            .map(|c| c.connected)
            .unwrap_or(false)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{what} did not connect to {remote}");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_idle(api: &SyncthingClient, what: &str) -> Result<FolderStatus> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = match api.folder_status(FOLDER).await {
            Ok(status) => status,
            Err(error) if format!("{error}").contains("no such folder") => {
                if Instant::now() >= deadline {
                    let folders = api
                        .config()
                        .await
                        .map(|c| c["folders"].clone())
                        .unwrap_or(Value::Null);
                    bail!("{what} folder never appeared: {error}; config folders={folders}");
                }
                sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if !status.error.is_empty() || status.state == "error" || status.errors > 0 {
            bail!(
                "{what} folder error: state={} error={} errors={} pullErrors={}",
                status.state,
                status.error,
                status.errors,
                status.pull_errors
            );
        }
        if status.is_idle() {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            bail!(
                "{what} folder did not become idle: state={} needFiles={}",
                status.state,
                status.need_files
            );
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_file(path: &Path, present: bool, what: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if path.exists() == present {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{what}: {} present={present}", path.display());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    Ok(())
}

async fn wait_folder_error(api: &SyncthingClient, needle: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = api.folder_status(FOLDER).await?;
        if status.error.contains(needle) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "folder did not report {needle:?}; state={} error={}",
                status.state,
                status.error
            );
        }
        sleep(Duration::from_millis(100)).await;
    }
}

struct Harness {
    temp: Option<TempDir>,
    hub: Syncthing,
    edge: Syncthing,
    status_child: Child,
    status: StatusClient,
    hub_library: PathBuf,
    edge_library: PathBuf,
    backup_marker: PathBuf,
    journal: PathBuf,
    base_ignores: Vec<String>,
    hub_gui: u16,
    edge_gui: u16,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.status_child.kill();
        let _ = self.status_child.wait();
        if std::env::var_os("PHOTOSYNC_KEEP_LIVE").is_some() {
            if let Some(temp) = self.temp.take() {
                let path = temp.keep();
                eprintln!("kept live test directory at {}", path.display());
            }
        }
    }
}

impl Harness {
    async fn start() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let base = temp.path().to_path_buf();
        let hub_home = base.join("hub-home");
        let edge_home = base.join("edge-home");
        let hub_library = base.join("hub-library");
        let edge_library = base.join("edge-library");
        for dir in [&hub_home, &edge_home, &hub_library, &edge_library] {
            fs::create_dir_all(dir)?;
        }
        fs::write(
            hub_library.join(".stignore"),
            "(?d).DS_Store\n(?d)**/.DS_Store\n",
        )?;

        let hub = Syncthing::start(&hub_home, "hub").await?;
        // Move the first instance off the default port before starting the
        // second, otherwise both briefly bind 0.0.0.0:22000.
        configure_options(&hub.api, hub.listen).await?;
        sleep(Duration::from_millis(500)).await;
        let edge = Syncthing::start(&edge_home, "edge").await?;
        configure_options(&edge.api, edge.listen).await?;

        configure_instance(
            &hub.api,
            &hub_library,
            &edge.id,
            "edge",
            edge.listen,
            hub.listen,
            true,
        )
        .await?;
        configure_instance(
            &edge.api,
            &edge_library,
            &hub.id,
            "hub",
            hub.listen,
            edge.listen,
            false,
        )
        .await?;
        wait_connected(&hub.api, &edge.id, "hub").await?;
        wait_connected(&edge.api, &hub.id, "edge").await?;
        wait_idle(&hub.api, "hub").await?;

        let status_port = free_port();
        let token = "photosync-live-status-token-0123456789".to_string();
        let backup_marker = base.join("last-backup");
        fs::write(&backup_marker, backup_time())?;
        let config = base.join("status.toml");
        fs::write(
            &config,
            format!(
                "bind = \"127.0.0.1:{status_port}\"\nroot = {:?}\nignore_file = \".stignore\"\nbackup_marker = {:?}\nsizes_cache_seconds = 0\nmax_entries = 10000\n",
                hub_library.display().to_string(),
                backup_marker.display().to_string()
            ),
        )?;
        let token_file = base.join("status.token");
        fs::write(&token_file, &token)?;
        let log = File::create(base.join("status.log"))?;
        let err = log.try_clone()?;
        let status_child = Command::new(env!("CARGO_BIN_EXE_photosync-status"))
            .arg("--config")
            .arg(&config)
            .arg("--token-file")
            .arg(&token_file)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .context("starting photosync-status")?;
        let status = StatusClient::new(&format!("http://127.0.0.1:{status_port}"), &token)?;
        wait_status(&status, base.join("status.log")).await?;
        let base_ignores = status.base_ignores().await?;

        let hub_gui = hub.gui;
        let edge_gui = edge.gui;
        Ok(Self {
            temp: Some(temp),
            hub,
            edge,
            status_child,
            status,
            hub_library,
            edge_library,
            backup_marker,
            journal: base.join("journal"),
            base_ignores,
            hub_gui,
            edge_gui,
        })
    }

    fn profile(&self) -> PathBuf {
        profile_path(&self.edge_library, DEVICE).unwrap()
    }

    fn hub_path(&self, relative: &str) -> PathBuf {
        self.hub_library.join(relative)
    }

    fn edge_path(&self, relative: &str) -> PathBuf {
        self.edge_library.join(relative)
    }

    async fn bootstrap(&self, active: &[&str]) -> Result<()> {
        let profile = Profile::new(
            DEVICE,
            self.base_ignores.clone(),
            vec![ALWAYS_ON.into()],
            active.iter().map(|p| p.to_string()).collect(),
        )?;
        let state = bootstrap(&self.edge_library, DEVICE, &profile)?;
        if matches!(
            state,
            photosync_core::profile::BootstrapState::Mismatch { .. }
        ) {
            bail!("live edge .stignore mismatch");
        }
        configure_instance(
            &self.edge.api,
            &self.edge_library,
            &self.hub.id,
            "hub",
            self.hub.listen,
            self.edge.listen,
            true,
        )
        .await?;
        wait_connected(&self.edge.api, &self.hub.id, "edge").await?;
        wait_idle(&self.edge.api, "edge").await?;
        Ok(())
    }

    fn set_profile(&self, active: &[&str]) -> Result<()> {
        let mut profile = read_profile(&self.profile(), &[ALWAYS_ON.into()])?;
        profile.base_ignores = self.base_ignores.clone();
        profile.always_on = vec![ALWAYS_ON.into()];
        profile.active = active.iter().map(|p| p.to_string()).collect();
        write_profile(&self.profile(), &profile)?;
        Ok(())
    }

    async fn scan_edge(&self, sub: Option<&str>) -> Result<()> {
        self.edge
            .api
            .scan_and_wait(
                FOLDER,
                sub,
                Duration::from_secs(30),
                Duration::from_millis(100),
            )
            .await?;
        Ok(())
    }

    async fn reload_profile(&self) -> Result<()> {
        self.scan_edge(Some(".stprofiles")).await?;
        // A .stprofiles scan reloads ignore patterns, but Syncthing also needs
        // a full folder scan to re-evaluate files that were previously ignored.
        self.scan_edge(None).await?;
        Ok(())
    }

    async fn scan_hub(&self) -> Result<()> {
        self.hub
            .api
            .scan_and_wait(
                FOLDER,
                None,
                Duration::from_secs(30),
                Duration::from_millis(100),
            )
            .await?;
        Ok(())
    }

    async fn wait_edge_ignores_absent(&self, paths: &[&str]) -> Result<()> {
        let lines: Vec<String> = paths
            .iter()
            .map(|p| format!("!/{}", photosync_core::profile::escape_glob(p)))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let ignores = self.edge.api.ignores(FOLDER).await?;
            if lines.iter().all(|line| !ignores.contains_line(line)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "edge still reports ignored project lines: {:?}",
                    ignores.expanded
                );
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_hub_profile_contains(&self, needle: &str) -> Result<()> {
        let path = self.hub_path(&format!(".stprofiles/{DEVICE}.txt"));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = fs::read_to_string(&path) {
                if text.contains(needle) {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                bail!("hub profile never contained {needle:?}: {}", path.display());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_edge_ignores_present(&self, paths: &[&str]) -> Result<()> {
        let lines: Vec<String> = paths
            .iter()
            .map(|p| format!("!/{}", photosync_core::profile::escape_glob(p)))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let ignores = self.edge.api.ignores(FOLDER).await?;
            if lines.iter().all(|line| ignores.contains_line(line)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!(
                    "edge did not reload the new project lines: {:?}",
                    ignores.expanded
                );
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    fn backup_time() -> String {
        (Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn write_backup(&self, value: Option<&str>) -> Result<()> {
        match value {
            Some(value) => fs::write(&self.backup_marker, format!("{value}\n"))?,
            None => {
                let _ = fs::remove_file(&self.backup_marker);
            }
        }
        Ok(())
    }

    fn release_context(&self, dry_run: bool) -> ReleaseContext {
        ReleaseContext {
            folder_id: FOLDER.into(),
            folder_root: self.edge_library.clone(),
            profile_path: self.profile(),
            device: DEVICE.into(),
            base_ignores: self.base_ignores.clone(),
            always_on: vec![ALWAYS_ON.into()],
            syncthing: Arc::new(self.edge.api.clone()),
            hub: Arc::new(self.status.clone()),
            timeouts: ReleaseTimeouts {
                idle: Duration::from_secs(30),
                ignores: Duration::from_secs(2),
                poll: Duration::from_millis(100),
            },
            dry_run,
        }
    }

    async fn release(&self, path: &str) -> Result<photosync_core::release::ReleaseRecord> {
        let project = Project::from_originals(path)?;
        let runner = ReleaseRunner::new(
            self.release_context(false),
            ReleaseJournal::new(&self.journal),
            project,
        )?;
        Ok(runner.run().await)
    }

    async fn release_with_stale_ignores(
        &self,
        path: &str,
        stale: Vec<String>,
    ) -> Result<photosync_core::release::ReleaseRecord> {
        let project = Project::from_originals(path)?;
        let inner = Arc::new(self.edge.api.clone());
        let syncthing = Arc::new(StaleIgnores {
            inner,
            stale,
            calls: AtomicUsize::new(0),
            stale_for: 1_000,
        });
        let mut ctx = self.release_context(false);
        ctx.syncthing = syncthing;
        let runner = ReleaseRunner::new(ctx, ReleaseJournal::new(&self.journal), project)?;
        Ok(runner.run().await)
    }
}

impl Harness {
    async fn prepare_project(&self, project: &str) -> Result<String> {
        if !self.profile().exists() {
            self.bootstrap(&[]).await?;
        }
        let published = project.replacen("originals/", "published/", 1);
        write_file(&self.hub_path(&format!("{project}/a.jpg")), b"release-me")
            .with_context(|| format!("writing hub file for {project}"))?;
        write_file(
            &self.hub_path(&format!("{published}/a.jpg")),
            b"release-pub",
        )
        .with_context(|| format!("writing hub published file for {project}"))?;
        self.scan_hub()
            .await
            .with_context(|| format!("scanning hub for {project}"))?;
        self.set_profile(&[project, &published])
            .with_context(|| format!("setting profile for {project}"))?;
        self.reload_profile()
            .await
            .with_context(|| format!("reloading profile for {project}"))?;
        self.wait_edge_ignores_present(&[project, &published])
            .await?;
        wait_file(
            &self.edge_path(&format!("{project}/a.jpg")),
            true,
            &format!("preparing {project}"),
        )
        .await?;
        self.write_backup(Some(&Harness::backup_time()))?;
        Ok(published)
    }

    async fn release_with_hub_fault(
        &self,
        path: &str,
        fault: HubFault,
    ) -> Result<photosync_core::release::ReleaseRecord> {
        let project = Project::from_originals(path)?;
        let mut ctx = self.release_context(false);
        ctx.hub = Arc::new(FaultyHub::new(Arc::new(self.status.clone()), fault));
        let runner = ReleaseRunner::new(ctx, ReleaseJournal::new(&self.journal), project)?;
        Ok(runner.run().await)
    }

    async fn release_with_syncthing(
        &self,
        path: &str,
        syncthing: Arc<dyn SyncthingApi>,
    ) -> Result<photosync_core::release::ReleaseRecord> {
        let project = Project::from_originals(path)?;
        let mut ctx = self.release_context(false);
        ctx.syncthing = syncthing;
        let runner = ReleaseRunner::new(ctx, ReleaseJournal::new(&self.journal), project)?;
        Ok(runner.run().await)
    }
}

async fn wait_status(status: &StatusClient, log: PathBuf) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match status.base_ignores().await {
            Ok(_) => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    bail!(
                        "status server did not start: {error}\n{}",
                        fs::read_to_string(log).unwrap_or_default()
                    );
                }
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn backup_time() -> String {
    (Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

struct StaleIgnores {
    inner: Arc<SyncthingClient>,
    stale: Vec<String>,
    calls: AtomicUsize,
    stale_for: usize,
}

#[async_trait]
impl SyncthingApi for StaleIgnores {
    async fn folder_status(&self, folder: &str) -> CoreResult<FolderStatus> {
        Ok(self.inner.folder_status(folder).await?)
    }
    async fn scan(&self, folder: &str, sub: Option<&str>) -> CoreResult<()> {
        Ok(self.inner.scan(folder, sub).await?)
    }
    async fn ignores(&self, folder: &str) -> CoreResult<Ignores> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.stale_for {
            Ok(Ignores {
                ignore: Vec::new(),
                expanded: self.stale.clone(),
            })
        } else {
            Ok(self.inner.ignores(folder).await?)
        }
    }
}

enum HubFault {
    Missing(String),
    Size(String, u64),
    Mtime(String, i64),
    StaleBackup,
    /// Return the normal tree for the first N calls, then an empty tree.
    LoseAfter(usize),
}

struct FaultyHub {
    inner: Arc<StatusClient>,
    fault: HubFault,
    calls: AtomicUsize,
}

impl FaultyHub {
    fn new(inner: Arc<StatusClient>, fault: HubFault) -> Self {
        Self {
            inner,
            fault,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl photosync_core::release::HubApi for FaultyHub {
    async fn tree(&self, prefix: &str) -> CoreResult<Vec<photosync_core::status_client::HubFile>> {
        if let HubFault::LoseAfter(limit) = &self.fault {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call >= *limit {
                return Ok(Vec::new());
            }
        }
        let mut files = self.inner.tree(prefix).await?;
        match &self.fault {
            HubFault::Missing(path) => files.retain(|file| &file.path != path),
            HubFault::Size(path, size) => {
                if let Some(file) = files.iter_mut().find(|file| &file.path == path) {
                    file.size = *size;
                }
            }
            HubFault::Mtime(path, mtime) => {
                if let Some(file) = files.iter_mut().find(|file| &file.path == path) {
                    file.mtime_ns = *mtime;
                }
            }
            _ => {}
        }
        Ok(files)
    }

    async fn backup(&self) -> CoreResult<photosync_core::status_client::BackupStatus> {
        if matches!(self.fault, HubFault::StaleBackup) {
            return Ok(photosync_core::status_client::BackupStatus {
                last_success: chrono::DateTime::from_timestamp(1_500_000_000, 0),
                marker_mtime_ns: Some(1_500_000_000_000_000_000),
            });
        }
        self.inner.backup().await
    }
}

struct FailScanOnProfile {
    inner: Arc<SyncthingClient>,
}

#[async_trait]
impl SyncthingApi for FailScanOnProfile {
    async fn folder_status(&self, folder: &str) -> CoreResult<FolderStatus> {
        self.inner.folder_status(folder).await
    }
    async fn scan(&self, folder: &str, sub: Option<&str>) -> CoreResult<()> {
        if sub == Some(photosync_core::profile::PROFILE_DIR) {
            return Err(photosync_core::Error::Syncthing(
                "injected profile scan failure".into(),
            ));
        }
        self.inner.scan(folder, sub).await
    }
    async fn ignores(&self, folder: &str) -> CoreResult<Ignores> {
        self.inner.ignores(folder).await
    }
}

fn project_files(project: &str, file: &str) -> (String, String) {
    (
        format!("{project}/{file}"),
        project.replacen("originals/", "published/", 1) + "/" + file,
    )
}

#[ignore = "requires two real Syncthing instances"]
#[tokio::test(flavor = "multi_thread")]
async fn selection_and_ignore_facts() -> Result<()> {
    let harness = Harness::start().await?;
    let a = "originals/Years/2024/2024.05.01 Coast";
    let a_pub = "published/Years/2024/2024.05.01 Coast";
    let b = "originals/Years/2024/2024.06.02 Hills";
    let b_pub = "published/Years/2024/2024.06.02 Hills";
    let c = "originals/Projects/Family";
    let c_pub = "published/Projects/Family";
    let d = "originals/Years/2024/2024.07.03 City";
    let d_pub = "published/Years/2024/2024.07.03 City";
    // Glob metacharacters must be escaped in the profile and still work.
    let e = "originals/Years/2024/Trip [2024] {a}?*";
    let e_pub = "published/Years/2024/Trip [2024] {a}?*";
    let inbox = "inbox/PixelMob/offload.jpg";
    for path in [
        format!("{a}/a.jpg"),
        format!("{a_pub}/a.jpg"),
        format!("{b}/b.jpg"),
        format!("{b_pub}/b.jpg"),
        format!("{c}/c.jpg"),
        format!("{c_pub}/c.jpg"),
        format!("{d}/d.jpg"),
        format!("{d_pub}/d.jpg"),
        format!("{e}/e.jpg"),
        format!("{e_pub}/e.jpg"),
        inbox.to_string(),
    ] {
        write_file(&harness.hub_path(&path), b"photo")?;
    }
    write_file(&harness.hub_path(&format!("{a}/.DS_Store")), b"x")?;
    harness.scan_hub().await?;

    harness.bootstrap(&[a, a_pub]).await?;
    harness.reload_profile().await?;
    wait_file(
        &harness.edge_path(&format!("{a}/a.jpg")),
        true,
        "fact 1 project A",
    )
    .await?;
    wait_file(
        &harness.edge_path(&format!("{a_pub}/a.jpg")),
        true,
        "fact 1 published A",
    )
    .await?;
    assert!(
        !harness.edge_path(&format!("{b}/b.jpg")).exists(),
        "fact 1: B must stay off the device"
    );
    assert!(
        harness.edge_path(inbox).exists(),
        "always-on inbox is active"
    );
    assert!(
        harness.edge_path("originals/Years/2024").is_dir(),
        "parent directories are created"
    );
    harness.wait_hub_profile_contains(a).await?;

    harness.set_profile(&[a, a_pub, b, b_pub])?;
    harness.reload_profile().await?;
    harness.wait_edge_ignores_present(&[b, b_pub]).await?;
    wait_file(
        &harness.edge_path(&format!("{b}/b.jpg")),
        true,
        "fact 2 project B",
    )
    .await?;
    harness.wait_hub_profile_contains(b).await?;

    harness.set_profile(&[a, a_pub])?;
    harness.reload_profile().await?;
    harness.wait_edge_ignores_absent(&[b, b_pub]).await?;
    fs::remove_dir_all(harness.edge_path(b))?;
    harness.scan_edge(None).await?;
    assert!(
        harness.hub_path(&format!("{b}/b.jpg")).exists(),
        "fact 3: deleting a released project locally must not propagate"
    );

    fs::remove_file(harness.edge_path(&format!("{a}/a.jpg")))?;
    harness.scan_edge(None).await?;
    wait_file(
        &harness.hub_path(&format!("{a}/a.jpg")),
        false,
        "fact 4 active deletion propagates",
    )
    .await?;

    // Fact 5: a missing include file stops the folder and deletes nothing.
    harness.set_profile(&[a, a_pub, c, c_pub])?;
    harness.reload_profile().await?;
    harness.wait_edge_ignores_present(&[c, c_pub]).await?;
    wait_file(
        &harness.edge_path(&format!("{c}/c.jpg")),
        true,
        "fact 5 project C",
    )
    .await?;
    let profile_text = fs::read_to_string(harness.profile())?;
    fs::remove_file(harness.profile())?;
    let _ = harness.edge.api.scan(FOLDER, Some(".stprofiles")).await;
    wait_folder_error(&harness.edge.api, "failed to load include file").await?;
    assert!(
        harness.edge_path(&format!("{c}/c.jpg")).exists(),
        "fact 5: missing include deletes nothing"
    );
    fs::write(harness.profile(), profile_text)?;
    harness.reload_profile().await?;
    wait_idle(&harness.edge.api, "edge after include restored").await?;
    assert!(harness.edge_path(&format!("{c}/c.jpg")).exists());

    // Fact 6: the device's browse view includes projects it does not hold.
    harness.scan_hub().await?;
    let browse = harness.edge.api.browse(FOLDER, "").await?;
    let mut flat = Vec::new();
    for entry in &browse {
        entry.flatten("", &mut flat);
    }
    assert!(
        flat.iter().any(|(path, _)| path == &format!("{d}/d.jpg")),
        "fact 6: edge browse should list the global tree, including {d}"
    );
    let dir = flat
        .iter()
        .find(|(path, _)| path == "originals/Years/2024")
        .map(|(_, entry)| *entry)
        .ok_or_else(|| anyhow!("browse did not return the 2024 directory"))?;
    assert_eq!(dir.size, 0, "fact 6: directory sizes come back as 0");

    // Facts 7 and 8 are explicitly not safety signals. Record what the
    // unreliable endpoints say so a failure is visible, and rely on the hub
    // disk tree everywhere in the gate.
    let completion = harness.edge.api.completion(FOLDER, &harness.hub.id).await?;
    let remoteneed = harness.edge.api.remoteneed(FOLDER, &harness.hub.id).await?;
    let hub_file = harness
        .hub
        .api
        .db_file(FOLDER, &format!("{b}/b.jpg"))
        .await
        .unwrap_or(Value::Null);
    eprintln!(
        "fact 7/8 observations: completion={completion} remoteneed={remoteneed} hub_file={hub_file}"
    );
    eprintln!(
        "harness ports: hub gui={} edge gui={}; temp={}",
        harness.hub_gui,
        harness.edge_gui,
        harness.temp.as_ref().unwrap().path().display()
    );
    Ok(())
}

#[ignore = "requires two real Syncthing instances"]
#[tokio::test(flavor = "multi_thread")]
async fn release_gate_live() -> Result<()> {
    let harness = Harness::start().await?;
    let a = "originals/Years/2025/2025.01.01 Release";
    let a_pub = harness.prepare_project(a).await?;
    let record = harness.release(a).await?;
    assert_eq!(
        record.phase,
        photosync_core::release::ReleasePhase::Completed,
        "{:?}",
        record.error
    );
    assert!(
        !harness.edge_path(a).exists(),
        "successful release deletes local files"
    );
    assert!(
        harness
            .status
            .tree(a)
            .await?
            .iter()
            .any(|f| f.path == format!("{a}/a.jpg")),
        "hub still has the released file"
    );
    assert_eq!(
        record
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Passed)
            .count(),
        6
    );
    let _ = a_pub;

    // Gate 1: the folder is in an error state before the release starts.
    let p1 = "originals/Years/2025/2025.02.01 FolderError";
    harness.prepare_project(p1).await?;
    let project = Project::from_originals(p1)?;
    let runner = ReleaseRunner::new(
        harness.release_context(false),
        ReleaseJournal::new(&harness.journal),
        project,
    )?;
    let profile_text = fs::read_to_string(harness.profile())?;
    fs::remove_file(harness.profile())?;
    let _ = harness.edge.api.scan(FOLDER, Some(".stprofiles")).await;
    wait_folder_error(&harness.edge.api, "failed to load include file").await?;
    let record = runner.run().await;
    assert!(
        matches!(
            record.phase,
            photosync_core::release::ReleasePhase::Aborted
                | photosync_core::release::ReleasePhase::Failed
        ),
        "folder error must block: {record:?}"
    );
    assert!(harness.edge_path(&format!("{p1}/a.jpg")).exists());
    fs::write(harness.profile(), profile_text)?;
    let _ = harness.edge.api.scan(FOLDER, Some(".stprofiles")).await;
    let _ = wait_idle(&harness.edge.api, "edge after folder error").await;

    // Gate 2: the hub is missing a file the device holds.
    let p2 = "originals/Years/2025/2025.03.01 Missing";
    harness.prepare_project(p2).await?;
    let record = harness
        .release_with_hub_fault(p2, HubFault::Missing(format!("{p2}/a.jpg")))
        .await?;
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("hub is missing"),
        "{record:?}"
    );
    assert!(harness.edge_path(&format!("{p2}/a.jpg")).exists());

    // Gate 2: a size mismatch.
    let p3 = "originals/Years/2025/2025.04.01 Size";
    harness.prepare_project(p3).await?;
    let record = harness
        .release_with_hub_fault(p3, HubFault::Size(format!("{p3}/a.jpg"), 1))
        .await?;
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("size mismatch"),
        "{record:?}"
    );
    assert!(harness.edge_path(&format!("{p3}/a.jpg")).exists());

    // Gate 2: an mtime mismatch.
    let p4 = "originals/Years/2025/2025.05.01 Time";
    harness.prepare_project(p4).await?;
    let record = harness
        .release_with_hub_fault(p4, HubFault::Mtime(format!("{p4}/a.jpg"), 1))
        .await?;
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("mtime mismatch"),
        "{record:?}"
    );
    assert!(harness.edge_path(&format!("{p4}/a.jpg")).exists());

    // Gate 3: the backup is older than the newest ctime.
    let p5 = "originals/Years/2025/2025.06.01 Backup";
    harness.prepare_project(p5).await?;
    let record = harness
        .release_with_hub_fault(p5, HubFault::StaleBackup)
        .await?;
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("not newer"),
        "{record:?}"
    );
    assert!(harness.edge_path(&format!("{p5}/a.jpg")).exists());

    // Gate 4: the profile scan fails.
    let p6 = "originals/Years/2025/2025.07.01 Scan";
    harness.prepare_project(p6).await?;
    let record = harness
        .release_with_syncthing(
            p6,
            Arc::new(FailScanOnProfile {
                inner: Arc::new(harness.edge.api.clone()),
            }),
        )
        .await?;
    assert!(
        matches!(
            record.phase,
            photosync_core::release::ReleasePhase::Aborted
                | photosync_core::release::ReleasePhase::Failed
        ),
        "profile scan failure must block: {record:?}"
    );
    assert!(harness.edge_path(&format!("{p6}/a.jpg")).exists());
    let profile = read_profile(&harness.profile(), &[ALWAYS_ON.into()])?;
    assert!(profile.active.contains(&p6.to_string()));

    // Gate 5: ignores are not reloaded after the profile rewrite.
    let p7 = "originals/Years/2025/2025.08.01 Ignores";
    harness.prepare_project(p7).await?;
    let stale = harness.edge.api.ignores(FOLDER).await?.expanded;
    let record = harness.release_with_stale_ignores(p7, stale).await?;
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("still reports"),
        "{record:?}"
    );
    assert!(harness.edge_path(&format!("{p7}/a.jpg")).exists());
    let profile = read_profile(&harness.profile(), &[ALWAYS_ON.into()])?;
    assert!(profile.active.contains(&p7.to_string()));

    // Gate 6: the hub loses files after local deletion. This is the loud case.
    let p8 = "originals/Years/2025/2025.09.01 Lost";
    harness.prepare_project(p8).await?;
    let record = harness
        .release_with_hub_fault(p8, HubFault::LoseAfter(2))
        .await?;
    assert_eq!(record.phase, photosync_core::release::ReleasePhase::Failed);
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("LOUD ERROR"),
        "{record:?}"
    );
    assert!(!harness.edge_path(p8).exists());
    Ok(())
}

#[ignore = "requires two real Syncthing instances"]
#[tokio::test(flavor = "multi_thread")]
async fn profile_scan_alone_pulls() -> Result<()> {
    let harness = Harness::start().await?;
    let a = "originals/Projects/Base";
    let a_pub = "published/Projects/Base";
    let b = "originals/Projects/Added";
    let b_pub = "published/Projects/Added";
    for path in [
        format!("{a}/a.jpg"),
        format!("{a_pub}/a.jpg"),
        format!("{b}/b.jpg"),
        format!("{b_pub}/b.jpg"),
    ] {
        write_file(&harness.hub_path(&path), b"x")?;
    }
    harness.scan_hub().await?;
    harness.bootstrap(&[a, a_pub]).await?;
    harness.reload_profile().await?;
    wait_file(&harness.edge_path(&format!("{a}/a.jpg")), true, "base").await?;
    harness.set_profile(&[a, a_pub, b, b_pub])?;
    harness.scan_edge(Some(".stprofiles")).await?;
    harness.wait_edge_ignores_present(&[b, b_pub]).await?;
    wait_file(
        &harness.edge_path(&format!("{b}/b.jpg")),
        true,
        "profile-scan-alone pull",
    )
    .await?;
    Ok(())
}

#[test]
fn helper_paths_are_relative() {
    assert_eq!(
        project_files("originals/Years/2024/A", "x.jpg"),
        (
            "originals/Years/2024/A/x.jpg".to_string(),
            "published/Years/2024/A/x.jpg".to_string()
        )
    );
    let _ = SocketAddr::from(([127, 0, 0, 1], 0));
}
