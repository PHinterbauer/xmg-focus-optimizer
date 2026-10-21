//! The small job protocol between the GUI (unelevated) and the worker
//! (elevated).
//!
//! Writing the job to a file and handing the file to a scheduled task keeps
//! UAC out of the interaction entirely after the one-time setup.

use serde::{Deserialize, Serialize};

use crate::backup::RestoreSource;
use crate::errors::{AppError, AppResult};
use crate::paths;
use crate::profile::Mode;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Job {
    /// Apply a mode (Quiet / Performance).
    Apply {
        mode: Mode,
        control_fans: bool,
        control_gpu: bool,
        control_display: bool,
        /// Power state at the time the job was queued. `None` means "query
        /// it yourself" (old job files, CLI). Auto-switch passes the value it
        /// just detected, because the worker's own query can lag behind the
        /// charger event and write mains values onto a battery scheme.
        #[serde(default)]
        on_ac: Option<bool>,
    },
    /// Restore settings from a backup source.
    Restore { source: RestoreSource },
    /// Capture the present state as a snapshot.
    Snapshot { reason: String, manual: bool },
    /// Capture the present state as a brand new initial backup.
    CreateFactoryBackup { note: String },
    /// Refresh the initial backup only if there is not one yet.
    EnsureFactoryBackup,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobResult {
    pub ok: bool,
    /// Short human-readable summary shown in the UI.
    pub summary: String,
    /// Individual lines for the detail view.
    pub details: Vec<String>,
    /// Anything that did not work, but did not make the job fail either.
    pub warnings: Vec<String>,
}

impl JobResult {
    pub fn fail(summary: impl Into<String>) -> Self {
        JobResult {
            ok: false,
            summary: summary.into(),
            details: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

pub fn write_job(job: &Job) -> AppResult<()> {
    let dir = paths::app_data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))?;
    let json = serde_json::to_vec_pretty(job).map_err(|e| AppError::Config(e.to_string()))?;
    std::fs::write(paths::job_path(), json)
        .map_err(|e| AppError::Io(format!("job.json: {e}")))
}

pub fn read_job() -> AppResult<Job> {
    let t = std::fs::read_to_string(paths::job_path())
        .map_err(|e| AppError::Io(format!("job.json: {e}")))?;
    serde_json::from_str(&t).map_err(|e| AppError::Config(format!("job.json: {e}")))
}

pub fn write_result(r: &JobResult) {
    let _ = std::fs::create_dir_all(paths::app_data_dir());
    if let Ok(json) = serde_json::to_vec_pretty(r) {
        let _ = std::fs::write(paths::job_result_path(), json);
    }
}

/// Takes the pending result, if any, leaving the slot empty.
pub fn take_result() -> Option<JobResult> {
    let p = paths::job_result_path();
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    serde_json::from_str(&t).ok()
}

/// Clears any leftovers so a stale result is never mistaken for a fresh one.
pub fn clear_pending() {
    let _ = std::fs::remove_file(paths::job_result_path());
    let _ = std::fs::remove_file(paths::job_path());
}

/// Removes only the queued job, keeping a written result in place.
///
/// The worker must use this after `write_result`, never `clear_pending`:
/// deleting the result in the same breath as writing it wins the race against
/// the GUI's poll almost every time, and the UI then waits on a file that will
/// never appear - the "Applying…" state never clears.
pub fn clear_job() {
    let _ = std::fs::remove_file(paths::job_path());
}