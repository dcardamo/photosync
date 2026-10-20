use crate::{
    ignore::IgnoreSet,
    local::{remove_project_path, walk_files},
    profile::{escape_glob, read_profile, write_profile, Profile},
    project::Project,
    status_client::{BackupStatus, HubFile, StatusClient},
    syncthing::{FolderStatus, Ignores, SyncthingClient},
    Error, Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

#[async_trait]
pub trait SyncthingApi: Send + Sync {
    async fn folder_status(&self, folder: &str) -> Result<FolderStatus>;
    async fn scan(&self, folder: &str, sub: Option<&str>) -> Result<()>;
    async fn ignores(&self, folder: &str) -> Result<Ignores>;
}

#[async_trait]
impl SyncthingApi for SyncthingClient {
    async fn folder_status(&self, folder: &str) -> Result<FolderStatus> {
        SyncthingClient::folder_status(self, folder).await
    }
    async fn scan(&self, folder: &str, sub: Option<&str>) -> Result<()> {
        SyncthingClient::scan(self, folder, sub).await
    }
    async fn ignores(&self, folder: &str) -> Result<Ignores> {
        SyncthingClient::ignores(self, folder).await
    }
}

#[async_trait]
pub trait HubApi: Send + Sync {
    async fn tree(&self, prefix: &str) -> Result<Vec<HubFile>>;
    async fn backup(&self) -> Result<BackupStatus>;
}

#[async_trait]
impl HubApi for StatusClient {
    async fn tree(&self, prefix: &str) -> Result<Vec<HubFile>> {
        StatusClient::tree(self, prefix).await
    }
    async fn backup(&self) -> Result<BackupStatus> {
        StatusClient::backup(self).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseStep {
    pub id: u8,
    pub name: String,
    pub status: StepStatus,
    pub detail: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleasePhase {
    Started,
    CheckingIdle,
    VerifyingFiles,
    CheckingBackup,
    RewritingProfile,
    VerifyingIgnores,
    Deleting,
    VerifyingHub,
    Completed,
    DryRunComplete,
    Aborted,
    Failed,
}

impl ReleasePhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::DryRunComplete | Self::Aborted | Self::Failed
        )
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Completed | Self::DryRunComplete)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedFile {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub id: String,
    pub project: Project,
    pub phase: ReleasePhase,
    pub steps: Vec<ReleaseStep>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub verified: Vec<VerifiedFile>,
    pub deletion_started: bool,
    pub dry_run: bool,
    pub profile_active_before: Vec<String>,
    pub profile_base: Vec<String>,
    pub always_on: Vec<String>,
}

impl ReleaseRecord {
    fn new(project: Project, profile: &Profile, dry_run: bool) -> Self {
        let now = Utc::now();
        let names = [
            (1, "Folder idle and project scanned"),
            (2, "Every local file exists on the hub"),
            (3, "Hub backup is newer than the files"),
            (4, "Profile rewritten and scanned"),
            (5, "Device ignores reloaded"),
            (6, "Local files deleted; hub still has them"),
        ];
        Self {
            id: Uuid::new_v4().to_string(),
            project,
            phase: ReleasePhase::Started,
            steps: names
                .into_iter()
                .map(|(id, name)| ReleaseStep {
                    id,
                    name: name.to_string(),
                    status: StepStatus::Pending,
                    detail: String::new(),
                    at: now,
                })
                .collect(),
            error: None,
            started_at: now,
            updated_at: now,
            verified: Vec::new(),
            deletion_started: false,
            dry_run,
            profile_active_before: profile.active.clone(),
            profile_base: profile.base_ignores.clone(),
            always_on: profile.always_on.clone(),
        }
    }

    pub fn step(&self, id: u8) -> Option<&ReleaseStep> {
        self.steps.iter().find(|s| s.id == id)
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseTimeouts {
    pub idle: Duration,
    pub ignores: Duration,
    pub poll: Duration,
}

impl Default for ReleaseTimeouts {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(180),
            ignores: Duration::from_secs(60),
            poll: Duration::from_millis(500),
        }
    }
}

pub struct ReleaseContext {
    pub folder_id: String,
    pub folder_root: PathBuf,
    pub profile_path: PathBuf,
    pub device: String,
    pub base_ignores: Vec<String>,
    pub always_on: Vec<String>,
    pub syncthing: Arc<dyn SyncthingApi>,
    pub hub: Arc<dyn HubApi>,
    pub timeouts: ReleaseTimeouts,
    pub dry_run: bool,
}

pub struct ReleaseJournal {
    dir: PathBuf,
}

impl ReleaseJournal {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, record: &ReleaseRecord) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{}.json", record.id));
        let bytes = serde_json::to_vec_pretty(record)?;
        crate::profile::write_atomic(&path, &bytes)?;
        self.prune(20)?;
        Ok(())
    }

    fn prune(&self, keep: usize) -> Result<()> {
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        files.push((modified, path));
                    }
                }
            }
        }
        files.sort_by_key(|(modified, _)| *modified);
        while files.len() > keep {
            let (_, path) = files.remove(0);
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    pub fn latest(&self) -> Result<Option<ReleaseRecord>> {
        Ok(self.list()?.into_iter().next())
    }

    pub fn list(&self) -> Result<Vec<ReleaseRecord>> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<ReleaseRecord>(&text) {
                    Ok(record) => records.push(record),
                    Err(_) => continue,
                },
                Err(_) => continue,
            }
        }
        records.sort_by_key(|record| std::cmp::Reverse(record.started_at));
        Ok(records)
    }

    pub fn load(&self, id: &str) -> Result<Option<ReleaseRecord>> {
        let path = self.dir.join(format!("{id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&fs::read_to_string(path)?)?))
    }
}

pub struct ReleaseRunner {
    ctx: ReleaseContext,
    journal: ReleaseJournal,
    record: ReleaseRecord,
}

impl ReleaseRunner {
    pub fn new(ctx: ReleaseContext, journal: ReleaseJournal, project: Project) -> Result<Self> {
        let profile = read_profile(&ctx.profile_path, &ctx.always_on)?;
        if !profile.active.contains(&project.originals) {
            return Err(Error::Release(format!(
                "{} is not active; refusing to release it",
                project.originals
            )));
        }
        if !profile.active.contains(&project.published) {
            return Err(Error::Release(format!(
                "{} is not active; refusing to release it",
                project.published
            )));
        }
        let record = ReleaseRecord::new(project, &profile, ctx.dry_run);
        Ok(Self {
            ctx,
            journal,
            record,
        })
    }

    pub fn record(&self) -> &ReleaseRecord {
        &self.record
    }

    /// Run every gate. A failure before deletion restores the profile and
    /// leaves the project active. A failure after deletion is loud and never
    /// re-activates or re-deletes.
    pub async fn run(mut self) -> ReleaseRecord {
        match self.run_inner().await {
            Ok(()) => self.record.clone(),
            Err(error) => {
                let message = error.to_string();
                if self.record.deletion_started {
                    self.record.phase = ReleasePhase::Failed;
                    self.record.error = Some(format!(
                        "deletion had started before this failure: {message}. The hub may have lost files; inspect the hub before doing anything else."
                    ));
                    self.mark(6, StepStatus::Failed, &message);
                    let _ = self.journal.save(&self.record);
                } else {
                    self.abort(message).await;
                }
                self.record.clone()
            }
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        self.record.phase = ReleasePhase::CheckingIdle;
        self.begin(1, "Checking that the folder is idle")?;
        self.check_folder_healthy().await?;
        self.scan_and_wait(Some(&self.record.project.originals))
            .await?;
        if self.record.project.published != self.record.project.originals {
            self.scan_and_wait(Some(&self.record.project.published))
                .await?;
        }
        self.pass(1, "folder idle, project scanned")?;

        self.record.phase = ReleasePhase::VerifyingFiles;
        self.begin(2, "Comparing device files with the hub")?;
        let verified = self.verify_files().await?;
        let detail = format!("{} file(s) matched the hub exactly", verified.len());
        self.record.verified = verified;
        self.pass(2, &detail)?;

        self.record.phase = ReleasePhase::CheckingBackup;
        self.begin(3, "Checking the hub backup marker")?;
        let backup_detail = self.check_backup().await?;
        self.pass(3, &backup_detail)?;

        self.record.phase = ReleasePhase::RewritingProfile;
        self.begin(4, "Removing the project from the profile")?;
        self.rewrite_profile().await?;
        self.pass(4, "profile written and .stprofiles scanned")?;

        self.record.phase = ReleasePhase::VerifyingIgnores;
        self.begin(5, "Waiting for the device to reload its ignores")?;
        self.wait_ignores_reloaded().await?;
        self.pass(5, "the device no longer lists the project")?;

        if self.ctx.dry_run {
            self.record.phase = ReleasePhase::DryRunComplete;
            self.mark(6, StepStatus::Skipped, "dry run: no files deleted");
            self.restore_profile().await?;
            self.journal.save(&self.record)?;
            return Ok(());
        }

        self.record.deletion_started = true;
        self.record.phase = ReleasePhase::Deleting;
        self.journal.save(&self.record)?;
        self.begin(6, "Deleting local files and confirming the hub")?;
        self.delete_and_verify().await?;
        self.pass(6, "local files deleted; hub still has every file")?;
        self.record.phase = ReleasePhase::Completed;
        self.record.error = None;
        self.journal.save(&self.record)?;
        Ok(())
    }

    async fn check_folder_healthy(&self) -> Result<()> {
        let status = self
            .ctx
            .syncthing
            .folder_status(&self.ctx.folder_id)
            .await?;
        if !status.error.is_empty() || status.errors > 0 || status.pull_errors > 0 {
            return Err(Error::Syncthing(format!(
                "folder is in an error state: {}",
                if status.error.is_empty() {
                    format!(
                        "errors={}, pullErrors={}",
                        status.errors, status.pull_errors
                    )
                } else {
                    status.error
                }
            )));
        }
        Ok(())
    }

    async fn scan_and_wait(&self, sub: Option<&str>) -> Result<()> {
        self.ctx.syncthing.scan(&self.ctx.folder_id, sub).await?;
        let deadline = tokio::time::Instant::now() + self.ctx.timeouts.idle;
        loop {
            let status = self
                .ctx
                .syncthing
                .folder_status(&self.ctx.folder_id)
                .await?;
            if status.state == "error"
                || !status.error.is_empty()
                || status.errors > 0
                || status.pull_errors > 0
            {
                return Err(Error::Syncthing(format!(
                    "folder entered an error state: {}",
                    if status.error.is_empty() {
                        format!(
                            "errors={}, pullErrors={}",
                            status.errors, status.pull_errors
                        )
                    } else {
                        status.error
                    }
                )));
            }
            if status.is_idle() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "folder did not become idle after {}s (state {})",
                    self.ctx.timeouts.idle.as_secs(),
                    status.state
                )));
            }
            tokio::time::sleep(self.ctx.timeouts.poll).await;
        }
    }

    async fn verify_files(&self) -> Result<Vec<VerifiedFile>> {
        let ignores = IgnoreSet::parse(self.ctx.base_ignores.iter().map(String::as_str))?;
        let mut verified = Vec::new();
        for path in [
            &self.record.project.originals,
            &self.record.project.published,
        ] {
            let local = walk_files(&self.ctx.folder_root, path, &ignores)?;
            if local.is_empty() {
                continue;
            }
            let hub = self.ctx.hub.tree(path).await?;
            let hub: HashMap<&str, &HubFile> =
                hub.iter().map(|file| (file.path.as_str(), file)).collect();
            for file in local {
                let Some(remote) = hub.get(file.path.as_str()) else {
                    return Err(Error::Release(format!(
                        "hub is missing {} (upload is not complete)",
                        file.path
                    )));
                };
                if remote.size != file.size {
                    return Err(Error::Release(format!(
                        "size mismatch for {}: device {}, hub {}",
                        file.path, file.size, remote.size
                    )));
                }
                if remote.mtime_ns != file.mtime_ns {
                    return Err(Error::Release(format!(
                        "mtime mismatch for {}: device {}, hub {}",
                        file.path, file.mtime_ns, remote.mtime_ns
                    )));
                }
                verified.push(VerifiedFile {
                    path: file.path,
                    size: file.size,
                    mtime_ns: file.mtime_ns,
                    ctime_ns: remote.ctime_ns,
                });
            }
        }
        Ok(verified)
    }

    async fn check_backup(&self) -> Result<String> {
        let backup = self.ctx.hub.backup().await?;
        let Some(last_success) = backup.last_success else {
            return Err(Error::Release(
                "the hub has no successful backup marker; refusing to release".into(),
            ));
        };
        let newest = self.record.verified.iter().map(|file| file.ctime_ns).max();
        if let Some(newest) = newest {
            let newest = DateTime::<Utc>::from_timestamp(
                newest.div_euclid(1_000_000_000),
                newest.rem_euclid(1_000_000_000) as u32,
            )
            .ok_or_else(|| Error::Release("hub ctime is out of range".into()))?;
            if last_success <= newest {
                return Err(Error::Release(format!(
                    "the last successful backup ({last_success}) is not newer than the newest file change ({newest}); run a backup first"
                )));
            }
            Ok(format!("backup {last_success} is newer than {newest}"))
        } else {
            Ok(format!(
                "backup {last_success} exists (project has no files on the device)"
            ))
        }
    }

    async fn rewrite_profile(&mut self) -> Result<()> {
        let mut profile = read_profile(&self.ctx.profile_path, &self.ctx.always_on)?;
        profile.base_ignores = self.ctx.base_ignores.clone();
        profile.always_on = self.ctx.always_on.clone();
        profile.remove_active(&self.record.project.originals)?;
        profile.remove_active(&self.record.project.published)?;
        write_profile(&self.ctx.profile_path, &profile)?;
        self.scan_and_wait(Some(crate::profile::PROFILE_DIR))
            .await?;
        Ok(())
    }

    async fn wait_ignores_reloaded(&self) -> Result<()> {
        let lines = [
            format!("!/{}", escape_glob(&self.record.project.originals)),
            format!("!/{}", escape_glob(&self.record.project.published)),
        ];
        let deadline = tokio::time::Instant::now() + self.ctx.timeouts.ignores;
        loop {
            let ignores = self.ctx.syncthing.ignores(&self.ctx.folder_id).await?;
            let still_there = lines.iter().find(|line| ignores.contains_line(line));
            if still_there.is_none() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "the device still reports {} in /rest/db/ignores after {}s; the profile was not reloaded",
                    still_there.unwrap(),
                    self.ctx.timeouts.ignores.as_secs()
                )));
            }
            tokio::time::sleep(self.ctx.timeouts.poll).await;
        }
    }

    async fn delete_and_verify(&mut self) -> Result<()> {
        remove_project_path(&self.ctx.folder_root, &self.record.project.originals)?;
        remove_project_path(&self.ctx.folder_root, &self.record.project.published)?;
        self.record.phase = ReleasePhase::VerifyingHub;
        self.journal.save(&self.record)?;
        self.ctx.syncthing.scan(&self.ctx.folder_id, None).await?;
        let deadline = tokio::time::Instant::now() + self.ctx.timeouts.idle;
        loop {
            let status = self
                .ctx
                .syncthing
                .folder_status(&self.ctx.folder_id)
                .await?;
            if status.state == "error"
                || !status.error.is_empty()
                || status.errors > 0
                || status.pull_errors > 0
            {
                return Err(Error::Syncthing(format!(
                    "folder entered an error state after deletion: {}",
                    if status.error.is_empty() {
                        format!(
                            "errors={}, pullErrors={}",
                            status.errors, status.pull_errors
                        )
                    } else {
                        status.error
                    }
                )));
            }
            if status.is_idle() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "folder did not become idle after deletion (state {})",
                    status.state
                )));
            }
            tokio::time::sleep(self.ctx.timeouts.poll).await;
        }
        self.confirm_hub_retained(&self.record.verified).await
    }

    async fn confirm_hub_retained(&self, verified: &[VerifiedFile]) -> Result<()> {
        if verified.is_empty() {
            return Ok(());
        }
        let mut hub = HashMap::new();
        for path in [
            &self.record.project.originals,
            &self.record.project.published,
        ] {
            for file in self.ctx.hub.tree(path).await? {
                hub.insert(file.path.clone(), file);
            }
        }
        let mut missing = Vec::new();
        for file in verified {
            match hub.get(&file.path) {
                Some(remote) if remote.size == file.size && remote.mtime_ns == file.mtime_ns => {}
                _ => missing.push(file.path.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(Error::Release(format!(
                "LOUD ERROR: the hub no longer has {} file(s) after local deletion, including {:?}. Stop and inspect the hub.",
                missing.len(),
                &missing[..missing.len().min(3)]
            )));
        }
        Ok(())
    }

    async fn abort(&mut self, reason: String) {
        self.record.phase = ReleasePhase::Aborted;
        self.record.error = Some(reason.clone());
        if let Some(step) = self
            .record
            .steps
            .iter_mut()
            .find(|s| s.status == StepStatus::Running)
        {
            step.status = StepStatus::Failed;
            step.detail = reason.clone();
            step.at = Utc::now();
        }
        if let Err(error) = self.restore_profile().await {
            self.record.phase = ReleasePhase::Failed;
            self.record.error = Some(format!(
                "{reason}. Restoring the profile also failed: {error}. The project may not be active; its local files and the hub are intact."
            ));
        }
        self.record.updated_at = Utc::now();
        let _ = self.journal.save(&self.record);
    }

    async fn restore_profile(&mut self) -> Result<()> {
        let mut profile = match read_profile(&self.ctx.profile_path, &self.ctx.always_on) {
            Ok(profile) => profile,
            Err(_) => Profile::new(
                &self.ctx.device,
                self.record.profile_base.clone(),
                self.record.always_on.clone(),
                self.record.profile_active_before.clone(),
            )?,
        };
        profile.base_ignores = self.record.profile_base.clone();
        profile.always_on = self.record.always_on.clone();
        profile.active = self.record.profile_active_before.clone();
        profile.add_active(&self.record.project.originals)?;
        profile.add_active(&self.record.project.published)?;
        write_profile(&self.ctx.profile_path, &profile)?;
        // The files are safe even if Syncthing is unavailable; the caller
        // still sees a loud failure because the project may not reload.
        self.scan_and_wait(Some(crate::profile::PROFILE_DIR))
            .await?;
        Ok(())
    }

    fn begin(&mut self, id: u8, detail: &str) -> Result<()> {
        self.mark(id, StepStatus::Running, detail);
        self.journal.save(&self.record)
    }

    fn pass(&mut self, id: u8, detail: &str) -> Result<()> {
        self.mark(id, StepStatus::Passed, detail);
        self.journal.save(&self.record)
    }

    fn mark(&mut self, id: u8, status: StepStatus, detail: &str) {
        if let Some(step) = self.record.steps.iter_mut().find(|s| s.id == id) {
            step.status = status;
            step.detail = detail.to_string();
            step.at = Utc::now();
        }
        self.record.updated_at = Utc::now();
    }
}

/// Inspect an interrupted release. Before deletion, restore the project and
/// mark the attempt aborted. After deletion, confirm the hub still has every
/// verified file; never re-activate and never delete again.
pub async fn recover(
    ctx: ReleaseContext,
    journal: ReleaseJournal,
    mut record: ReleaseRecord,
) -> ReleaseRecord {
    if record.phase.is_terminal() {
        return record;
    }
    let mut runner = ReleaseRunner {
        ctx,
        journal,
        record: record.clone(),
    };
    if record.deletion_started {
        record.phase = ReleasePhase::VerifyingHub;
        record.updated_at = Utc::now();
        match runner.confirm_hub_retained(&record.verified).await {
            Ok(()) => {
                record.phase = ReleasePhase::Completed;
                record.error = None;
                if let Some(step) = record.steps.iter_mut().find(|s| s.id == 6) {
                    step.status = StepStatus::Passed;
                    step.detail =
                        "recovered after interruption; hub still has every verified file".into();
                    step.at = Utc::now();
                }
            }
            Err(error) => {
                record.phase = ReleasePhase::Failed;
                record.error = Some(format!(
                    "interrupted while deleting; verifying the hub failed: {error}"
                ));
                if let Some(step) = record.steps.iter_mut().find(|s| s.id == 6) {
                    step.status = StepStatus::Failed;
                    step.detail = record.error.clone().unwrap_or_default();
                    step.at = Utc::now();
                }
            }
        }
    } else {
        record.phase = ReleasePhase::Aborted;
        record.error =
            Some("the app was interrupted before deletion; the project was left active".into());
        if let Some(step) = record
            .steps
            .iter_mut()
            .find(|s| s.status == StepStatus::Running)
        {
            step.status = StepStatus::Failed;
            step.detail = "interrupted before deletion".into();
            step.at = Utc::now();
        }
        if let Err(error) = runner.restore_profile().await {
            record.phase = ReleasePhase::Failed;
            record.error = Some(format!(
                "interrupted before deletion and profile restoration failed: {error}. Local files and the hub are intact."
            ));
        }
    }
    record.updated_at = Utc::now();
    let _ = runner.journal.save(&record);
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        profile::{profile_path, write_profile},
        project::ProjectLayout,
    };
    use std::{
        fs::File,
        io::Write,
        sync::Mutex,
        time::{Duration, SystemTime},
    };

    const MTIME_NS: i64 = 1_600_000_000_000_000_000;
    const CTIME_NS: i64 = 1_600_000_000_000_000_100;

    #[derive(Default)]
    struct FakeSyncthing {
        error: Mutex<Option<String>>,
        state: Mutex<String>,
        ignores_line: Mutex<Vec<String>>,
        reload_on_profile_scan: Mutex<bool>,
    }

    impl FakeSyncthing {
        fn new() -> Self {
            Self {
                error: Mutex::new(None),
                state: Mutex::new("idle".into()),
                ignores_line: Mutex::new(vec![
                    "!/originals/Projects/A".into(),
                    "!/published/Projects/A".into(),
                ]),
                reload_on_profile_scan: Mutex::new(true),
            }
        }
    }

    #[async_trait]
    impl SyncthingApi for FakeSyncthing {
        async fn folder_status(&self, _folder: &str) -> Result<FolderStatus> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(Error::Syncthing(error));
            }
            Ok(FolderStatus {
                state: self.state.lock().unwrap().clone(),
                ..Default::default()
            })
        }
        async fn scan(&self, _folder: &str, sub: Option<&str>) -> Result<()> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(Error::Syncthing(error));
            }
            if sub == Some(crate::profile::PROFILE_DIR)
                && *self.reload_on_profile_scan.lock().unwrap()
            {
                self.ignores_line.lock().unwrap().clear();
            }
            Ok(())
        }
        async fn ignores(&self, _folder: &str) -> Result<Ignores> {
            Ok(Ignores {
                ignore: vec!["#include .stprofiles/pixel.txt".into()],
                expanded: self.ignores_line.lock().unwrap().clone(),
            })
        }
    }

    struct FakeHub {
        files: Mutex<Vec<HubFile>>,
        backup: Mutex<BackupStatus>,
        drop_after_delete: Mutex<bool>,
        arm_drop_on_backup: Mutex<bool>,
    }

    impl FakeHub {
        fn new(files: Vec<HubFile>) -> Self {
            Self {
                files: Mutex::new(files),
                backup: Mutex::new(BackupStatus {
                    last_success: DateTime::from_timestamp(1_700_000_000, 0),
                    marker_mtime_ns: Some(1_700_000_000_000_000_000),
                }),
                drop_after_delete: Mutex::new(false),
                arm_drop_on_backup: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl HubApi for FakeHub {
        async fn tree(&self, prefix: &str) -> Result<Vec<HubFile>> {
            if *self.drop_after_delete.lock().unwrap() {
                self.files.lock().unwrap().clear();
            }
            Ok(self
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|f| {
                    f.path
                        .starts_with(&format!("{}/", prefix.trim_matches('/')))
                })
                .cloned()
                .collect())
        }
        async fn backup(&self) -> Result<BackupStatus> {
            if *self.arm_drop_on_backup.lock().unwrap() {
                *self.drop_after_delete.lock().unwrap() = true;
            }
            Ok(self.backup.lock().unwrap().clone())
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        profile_path: PathBuf,
        project: Project,
        record_dir: PathBuf,
        fake_sync: Arc<FakeSyncthing>,
        fake_hub: Arc<FakeHub>,
    }

    fn fixture(hub_files: Vec<(String, u64, i64)>, backup: Option<DateTime<Utc>>) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("photos");
        fs::create_dir_all(root.join("originals/Projects/A")).unwrap();
        fs::create_dir_all(root.join(".stprofiles")).unwrap();
        write_file(&root.join("originals/Projects/A/a.jpg"), b"abc");
        let project = Project::from_originals("originals/Projects/A").unwrap();
        let profile = Profile::new(
            "pixel",
            vec!["(?d).DS_Store".into()],
            vec!["inbox/PixelMob".into()],
            vec![project.originals.clone(), project.published.clone()],
        )
        .unwrap();
        let profile_path = profile_path(&root, "pixel").unwrap();
        write_profile(&profile_path, &profile).unwrap();
        fs::write(
            root.join(".stignore"),
            crate::profile::stignore_text("pixel").unwrap(),
        )
        .unwrap();
        let files = hub_files
            .into_iter()
            .map(|(path, size, mtime_ns)| HubFile {
                path,
                size,
                mtime_ns,
                ctime_ns: CTIME_NS,
            })
            .collect();
        let fake_hub = Arc::new(FakeHub::new(files));
        fake_hub.backup.lock().unwrap().last_success = backup;
        Fixture {
            record_dir: tmp.path().join("journals"),
            _tmp: tmp,
            root,
            profile_path,
            project,
            fake_sync: Arc::new(FakeSyncthing::new()),
            fake_hub,
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        let mut file = File::create(path).unwrap();
        file.write_all(bytes).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_nanos(MTIME_NS as u64))
            .unwrap();
    }

    fn context(fixture: &Fixture) -> ReleaseContext {
        ReleaseContext {
            folder_id: "photos".into(),
            folder_root: fixture.root.clone(),
            profile_path: fixture.profile_path.clone(),
            device: "pixel".into(),
            base_ignores: vec!["(?d).DS_Store".into()],
            always_on: vec!["inbox/PixelMob".into()],
            syncthing: fixture.fake_sync.clone(),
            hub: fixture.fake_hub.clone(),
            timeouts: ReleaseTimeouts {
                idle: Duration::from_millis(200),
                ignores: Duration::from_millis(200),
                poll: Duration::from_millis(5),
            },
            dry_run: false,
        }
    }

    fn runner(fixture: &Fixture) -> ReleaseRunner {
        ReleaseRunner::new(
            context(fixture),
            ReleaseJournal::new(fixture.record_dir.clone()),
            fixture.project.clone(),
        )
        .unwrap()
    }

    fn assert_left_active(fixture: &Fixture, record: &ReleaseRecord) {
        assert!(matches!(
            record.phase,
            ReleasePhase::Aborted | ReleasePhase::Failed
        ));
        assert!(
            fixture.root.join("originals/Projects/A/a.jpg").exists(),
            "local file must survive an aborted release"
        );
        let profile = read_profile(&fixture.profile_path, &["inbox/PixelMob".into()]).unwrap();
        assert!(profile.active.contains(&fixture.project.originals));
    }

    #[tokio::test]
    async fn successful_release_deletes_locally_and_keeps_hub_copy() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        let record = runner(&fixture).run().await;
        assert_eq!(record.phase, ReleasePhase::Completed, "{:?}", record.error);
        assert!(!fixture.root.join("originals/Projects/A").exists());
        assert!(fixture.fake_hub.files.lock().unwrap()[0]
            .path
            .ends_with("a.jpg"));
        assert_eq!(
            record
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Passed)
                .count(),
            6
        );
    }

    #[tokio::test]
    async fn hub_missing_file_blocks_and_leaves_project_active() {
        let fixture = fixture(
            vec![],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        let record = runner(&fixture).run().await;
        assert_left_active(&fixture, &record);
        assert!(record.error.unwrap().contains("hub is missing"));
    }

    #[tokio::test]
    async fn size_and_mtime_mismatches_block() {
        for (size, mtime, needle) in [
            (4, MTIME_NS, "size mismatch"),
            (3, MTIME_NS - 1, "mtime mismatch"),
        ] {
            let fixture = fixture(
                vec![("originals/Projects/A/a.jpg".into(), size, mtime)],
                Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            );
            let record = runner(&fixture).run().await;
            assert_left_active(&fixture, &record);
            assert!(record.error.unwrap().contains(needle));
        }
    }

    #[tokio::test]
    async fn stale_backup_blocks() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_500_000_000, 0).unwrap()),
        );
        let record = runner(&fixture).run().await;
        assert_left_active(&fixture, &record);
        assert!(record.error.unwrap().contains("not newer"));
    }

    #[tokio::test]
    async fn missing_backup_blocks() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            None,
        );
        let record = runner(&fixture).run().await;
        assert_left_active(&fixture, &record);
        assert!(record.error.unwrap().contains("no successful backup"));
    }

    #[tokio::test]
    async fn ignores_not_reloaded_blocks() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        *fixture.fake_sync.reload_on_profile_scan.lock().unwrap() = false;
        let record = runner(&fixture).run().await;
        assert_left_active(&fixture, &record);
        assert!(record.error.unwrap().contains("still reports"));
    }

    #[tokio::test]
    async fn folder_error_blocks() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        *fixture.fake_sync.error.lock().unwrap() = Some("database is locked".into());
        let record = runner(&fixture).run().await;
        assert_left_active(&fixture, &record);
    }

    #[tokio::test]
    async fn hub_losing_files_after_deletion_is_loud() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        *fixture.fake_hub.arm_drop_on_backup.lock().unwrap() = true;
        let record = runner(&fixture).run().await;
        assert_eq!(record.phase, ReleasePhase::Failed);
        assert!(record.error.unwrap().contains("LOUD ERROR"));
        assert!(!fixture.root.join("originals/Projects/A").exists());
    }

    #[tokio::test]
    async fn dry_run_restores_the_project() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        let mut ctx = context(&fixture);
        ctx.dry_run = true;
        let record = ReleaseRunner::new(
            ctx,
            ReleaseJournal::new(fixture.record_dir.clone()),
            fixture.project.clone(),
        )
        .unwrap()
        .run()
        .await;
        assert_eq!(
            record.phase,
            ReleasePhase::DryRunComplete,
            "{:?}",
            record.error
        );
        assert!(fixture.root.join("originals/Projects/A/a.jpg").exists());
        let profile = read_profile(&fixture.profile_path, &["inbox/PixelMob".into()]).unwrap();
        assert!(profile.active.contains(&fixture.project.originals));
    }

    #[tokio::test]
    async fn recovery_before_deletion_restores_profile() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        let runner = runner(&fixture);
        let mut record = runner.record().clone();
        record.phase = ReleasePhase::VerifyingIgnores;
        let record = recover(
            context(&fixture),
            ReleaseJournal::new(fixture.record_dir.clone()),
            record,
        )
        .await;
        assert_eq!(record.phase, ReleasePhase::Aborted);
        let profile = read_profile(&fixture.profile_path, &["inbox/PixelMob".into()]).unwrap();
        assert!(profile.active.contains(&fixture.project.originals));
        assert!(fixture.root.join("originals/Projects/A/a.jpg").exists());
    }

    #[tokio::test]
    async fn recovery_after_deletion_confirms_the_hub() {
        let fixture = fixture(
            vec![("originals/Projects/A/a.jpg".into(), 3, MTIME_NS)],
            Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        );
        let mut record = runner(&fixture).record().clone();
        record.deletion_started = true;
        record.verified = vec![VerifiedFile {
            path: "originals/Projects/A/a.jpg".into(),
            size: 3,
            mtime_ns: MTIME_NS,
            ctime_ns: CTIME_NS,
        }];
        record.phase = ReleasePhase::Deleting;
        let record = recover(
            context(&fixture),
            ReleaseJournal::new(fixture.record_dir.clone()),
            record,
        )
        .await;
        assert_eq!(record.phase, ReleasePhase::Completed, "{:?}", record.error);
    }

    #[tokio::test]
    async fn custom_layout_uses_configured_roots() {
        let layout = ProjectLayout {
            originals: "library".into(),
            years: "ByYear".into(),
            projects: "Collections".into(),
            published: "exports".into(),
        };
        let project = Project::from_originals_in("library/Collections/A", &layout).unwrap();
        assert_eq!(project.published, "exports/Collections/A");
    }
}
