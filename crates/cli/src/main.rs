use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use photosync_core::{
    profile::{bootstrap, profile_path, read_profile, write_profile, BootstrapState, Profile},
    project::{Project, ProjectLayout},
    release::{
        ReleaseContext, ReleaseJournal, ReleasePhase, ReleaseRunner, ReleaseTimeouts, StepStatus,
    },
    settings::Settings,
    status_client::StatusClient,
    syncthing::SyncthingClient,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, Parser)]
#[command(about = "Developer CLI for photosync")]
struct Cli {
    #[arg(
        long,
        env = "PHOTOSYNC_SYNCTHING_URL",
        default_value = "http://127.0.0.1:8384"
    )]
    syncthing_url: String,
    #[arg(long, env = "PHOTOSYNC_API_KEY")]
    api_key: Option<String>,
    #[arg(long, env = "PHOTOSYNC_FOLDER_ID", default_value = "photos")]
    folder_id: String,
    #[arg(long, env = "PHOTOSYNC_DEVICE")]
    device: String,
    #[arg(long, env = "PHOTOSYNC_ROOT")]
    root: PathBuf,
    #[arg(long, env = "PHOTOSYNC_STATUS_URL")]
    status_url: Option<String>,
    #[arg(long, env = "PHOTOSYNC_STATUS_TOKEN")]
    status_token: Option<String>,
    #[arg(
        long = "always-on",
        env = "PHOTOSYNC_ALWAYS_ON",
        default_value = "inbox/PixelMob"
    )]
    always_on: Vec<String>,
    #[arg(long, env = "PHOTOSYNC_HEADROOM", default_value_t = 10)]
    headroom_percent: u8,
    #[arg(long, env = "PHOTOSYNC_JOURNAL_DIR", default_value = ".data/journal")]
    journal_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the active projects in the profile.
    List,
    /// Add a project and its published twin to the profile, then scan.
    Activate { path: String },
    /// Run every release gate without deleting anything.
    Release {
        path: String,
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let layout = ProjectLayout::default();
    let settings = Settings {
        syncthing_url: cli.syncthing_url.clone(),
        folder_id: cli.folder_id.clone(),
        device_name: cli.device.clone(),
        folder_root: cli.root.display().to_string(),
        status_server_url: cli.status_url.clone().unwrap_or_default(),
        headroom_percent: cli.headroom_percent,
        always_on: cli.always_on.clone(),
        layout: layout.clone(),
        cached_base_ignores: Vec::new(),
        journal_dir: cli.journal_dir.display().to_string(),
    };
    settings.validate().context("invalid settings")?;
    let profile_path = profile_path(&cli.root, &cli.device)?;

    match cli.command {
        Command::List => list(&cli, &profile_path)?,
        Command::Activate { ref path } => {
            activate(&cli, &settings, &profile_path, &layout, path).await?;
        }
        Command::Release { ref path, dry_run } => {
            if !dry_run {
                bail!(
                    "the CLI can only run a release dry-run; use the Android app to delete files"
                );
            }
            release_dry_run(&cli, &settings, &profile_path, &layout, path).await?;
        }
    }
    Ok(())
}

fn list(cli: &Cli, profile_path: &std::path::Path) -> Result<()> {
    if !profile_path.exists() {
        println!("no profile at {}", profile_path.display());
        return Ok(());
    }
    let profile = read_profile(profile_path, &cli.always_on)?;
    println!(
        "base ignores: {}",
        profile
            .base_ignores
            .iter()
            .filter(|l| !l.trim().is_empty())
            .count()
    );
    println!("always on:");
    for path in &profile.always_on {
        println!("  {path}");
    }
    println!("active:");
    for path in &profile.active {
        match Project::from_originals(path) {
            Ok(project) => println!("  {}  ({})", project.name, project.originals),
            Err(_) => println!("  {path}"),
        }
    }
    Ok(())
}

async fn activate(
    cli: &Cli,
    settings: &Settings,
    profile_path: &std::path::Path,
    layout: &ProjectLayout,
    path: &str,
) -> Result<()> {
    let project = Project::from_originals_in(path, layout)?;
    let base = fetch_base_ignores(cli).await?;
    let profile = if profile_path.exists() {
        read_profile(profile_path, &cli.always_on)?
    } else {
        let profile = Profile::new(&cli.device, base.clone(), cli.always_on.clone(), Vec::new())?;
        let state = bootstrap(&cli.root, &cli.device, &profile)?;
        if let BootstrapState::Mismatch { existing } = state {
            bail!("existing .stignore does not match photosync:\n{existing}");
        }
        profile
    };
    let mut profile = profile;
    profile.base_ignores = base;
    profile.always_on = cli.always_on.clone();
    profile.add_active(&project.originals)?;
    profile.add_active(&project.published)?;
    write_profile(profile_path, &profile)?;
    let syncthing = syncthing(cli)?;
    syncthing
        .scan_and_wait(
            &settings.folder_id,
            Some(photosync_core::profile::PROFILE_DIR),
            Duration::from_secs(120),
            Duration::from_millis(500),
        )
        .await?;
    // A .stprofiles scan reloads ignores; a full scan re-evaluates files that
    // were previously ignored so the newly active project starts pulling.
    syncthing
        .scan_and_wait(
            &settings.folder_id,
            None,
            Duration::from_secs(120),
            Duration::from_millis(500),
        )
        .await?;
    println!("activated {} and {}", project.originals, project.published);
    Ok(())
}

async fn release_dry_run(
    cli: &Cli,
    settings: &Settings,
    profile_path: &std::path::Path,
    layout: &ProjectLayout,
    path: &str,
) -> Result<()> {
    let project = Project::from_originals_in(path, layout)?;
    let base = fetch_base_ignores(cli).await?;
    let syncthing = Arc::new(syncthing(cli)?);
    let hub = Arc::new(status_client(cli)?);
    let ctx = ReleaseContext {
        folder_id: settings.folder_id.clone(),
        folder_root: settings.root(),
        profile_path: profile_path.to_path_buf(),
        device: cli.device.clone(),
        base_ignores: base,
        always_on: cli.always_on.clone(),
        syncthing,
        hub,
        timeouts: ReleaseTimeouts {
            idle: Duration::from_secs(180),
            ignores: Duration::from_secs(60),
            poll: Duration::from_millis(500),
        },
        dry_run: true,
    };
    let journal = ReleaseJournal::new(&cli.journal_dir);
    let runner = ReleaseRunner::new(ctx, journal, project)?;
    let record = runner.run().await;
    for step in &record.steps {
        let status = match step.status {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Passed => "passed",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
        };
        println!("{} [{}] {}: {}", step.id, status, step.name, step.detail);
    }
    println!("phase: {:?}", record.phase);
    if let Some(error) = &record.error {
        println!("error: {error}");
    }
    if record.phase != ReleasePhase::DryRunComplete {
        bail!("release dry-run did not pass");
    }
    Ok(())
}

fn syncthing(cli: &Cli) -> Result<SyncthingClient> {
    let api_key = cli
        .api_key
        .as_deref()
        .context("--api-key or PHOTOSYNC_API_KEY is required")?;
    Ok(SyncthingClient::new(&cli.syncthing_url, api_key)?)
}

fn status_client(cli: &Cli) -> Result<StatusClient> {
    let url = cli
        .status_url
        .as_deref()
        .context("--status-url is required")?;
    let token = cli
        .status_token
        .as_deref()
        .context("--status-token is required")?;
    Ok(StatusClient::new(url, token)?)
}

async fn fetch_base_ignores(cli: &Cli) -> Result<Vec<String>> {
    Ok(status_client(cli)?.base_ignores().await?)
}
