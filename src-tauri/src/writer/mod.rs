//! The burn engine. Runs with root/Administrator rights.

pub mod copy;
pub mod raw;
pub mod windows_iso;

use crate::iso::WriteMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub iso: String,
    /// Linux: /dev/sdX   Windows: \\.\PhysicalDriveN
    pub device: String,
    /// Windows disk number
    pub number: Option<u32>,
    pub mode: WriteMode,
    pub verify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Progress {
    /// prepare | format | write | copy | split | verify | flush | done | error
    pub stage: String,
    pub message: String,
    pub done: u64,
    pub total: u64,
}

impl Progress {
    pub fn new(stage: &str, message: impl Into<String>, done: u64, total: u64) -> Self {
        Progress { stage: stage.into(), message: message.into(), done, total }
    }
}

pub type Reporter<'a> = &'a mut dyn FnMut(Progress);

pub fn burn(job: &Job, report: Reporter) -> anyhow::Result<()> {
    safety_check(job)?;
    match job.mode {
        WriteMode::Raw => raw::write(job, report)?,
        WriteMode::Windows => windows_iso::write(job, report)?,
    }
    report(Progress::new("done", "All done! You can remove the USB drive.", 1, 1));
    Ok(())
}

/// Re-checks, with elevated rights, that the target really is a removable drive
/// and not a system disk. Never trust the caller blindly.
fn safety_check(job: &Job) -> anyhow::Result<()> {
    let iso_size = std::fs::metadata(&job.iso)
        .map_err(|e| anyhow::anyhow!("Cannot read the ISO file: {}", e))?
        .len();
    let drives = crate::drives::list()?;
    let drive = drives
        .iter()
        .find(|d| d.device == job.device)
        .ok_or_else(|| anyhow::anyhow!("{} is not a removable USB drive (or it was unplugged). Nothing was written.", job.device))?;
    if job.mode == WriteMode::Raw && iso_size > drive.size {
        anyhow::bail!("The USB drive is too small for this image ({} needed).", human(iso_size));
    }
    if job.mode == WriteMode::Windows && iso_size > drive.size {
        anyhow::bail!("The USB drive is too small for this Windows image.");
    }
    Ok(())
}

pub fn human(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", v, units[i]) }
}
