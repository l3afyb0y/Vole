use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::Value;
use which::which;

#[derive(Debug, Clone)]
pub enum SnapshotProvider {
    Btrfs { source: PathBuf },
    TimeshiftBtrfs,
}

#[derive(Debug, Clone)]
pub struct SnapshotSupport {
    pub label: String,
    pub provider: SnapshotProvider,
}

#[derive(Debug, Clone)]
pub struct SnapshotOutcome {
    pub provider: String,
    pub location: Option<PathBuf>,
}

impl SnapshotOutcome {
    pub fn display(&self) -> String {
        if let Some(path) = &self.location {
            format!("{} snapshot at {}", self.provider, path.display())
        } else {
            format!("{} snapshot created", self.provider)
        }
    }
}

pub fn detect(home: &Path) -> Option<SnapshotSupport> {
    detect_btrfs(home).or_else(detect_timeshift_btrfs)
}

pub fn create_snapshot(support: &SnapshotSupport) -> Result<SnapshotOutcome> {
    match &support.provider {
        SnapshotProvider::Btrfs { source } => create_btrfs_snapshot(source),
        SnapshotProvider::TimeshiftBtrfs => create_timeshift_snapshot(),
    }
}

fn detect_btrfs(home: &Path) -> Option<SnapshotSupport> {
    if which("btrfs").is_err() {
        return None;
    }
    if !home.exists() {
        return None;
    }

    let status = Command::new("btrfs")
        .args(["subvolume", "show"])
        .arg(home)
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    Some(SnapshotSupport {
        label: "Btrfs (home)".to_string(),
        provider: SnapshotProvider::Btrfs {
            source: home.to_path_buf(),
        },
    })
}

fn detect_timeshift_btrfs() -> Option<SnapshotSupport> {
    if which("timeshift").is_err() {
        return None;
    }

    let config_paths = ["/etc/timeshift/timeshift.json", "/etc/timeshift.json"];
    for path in config_paths {
        if let Ok(data) = fs::read_to_string(path) {
            if timeshift_btrfs_enabled(&data) {
                return Some(SnapshotSupport {
                    label: "Timeshift (Btrfs)".to_string(),
                    provider: SnapshotProvider::TimeshiftBtrfs,
                });
            }
        }
    }

    None
}

fn timeshift_btrfs_enabled(data: &str) -> bool {
    if let Ok(json) = serde_json::from_str::<Value>(data) {
        for key in ["snapshot_type", "backup_type", "mode"] {
            if let Some(value) = json.get(key).and_then(|val| val.as_str()) {
                if value.to_lowercase().contains("btrfs") {
                    return true;
                }
            }
        }
    }

    data.to_lowercase().contains("btrfs")
}

fn create_btrfs_snapshot(source: &Path) -> Result<SnapshotOutcome> {
    let snapshot_dir = source.join(".local/share/vole/snapshots");
    fs::create_dir_all(&snapshot_dir)
        .with_context(|| format!("Failed to create {}", snapshot_dir.display()))?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let name = format!("vole-clean-{}", ts);
    let dest = snapshot_dir.join(name);

    let status = Command::new("btrfs")
        .args(["subvolume", "snapshot", "-r"])
        .arg(source)
        .arg(&dest)
        .status()
        .context("Failed to run btrfs snapshot")?;

    if !status.success() {
        bail!("btrfs snapshot command failed");
    }

    Ok(SnapshotOutcome {
        provider: "Btrfs".to_string(),
        location: Some(dest),
    })
}

fn create_timeshift_snapshot() -> Result<SnapshotOutcome> {
    let status = Command::new("timeshift")
        .args(["--create", "--comments", "Vole clean", "--tags", "O"])
        .status()
        .context("Failed to run timeshift")?;

    if !status.success() {
        bail!("timeshift snapshot command failed");
    }

    Ok(SnapshotOutcome {
        provider: "Timeshift".to_string(),
        location: None,
    })
}
