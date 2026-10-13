//! The elevated half of the app.
//!
//! Everything that touches power settings, the registry or the GPU runs here.
//! The GUI never needs elevation itself: it hands a job to this worker, which
//! the scheduled task starts with full rights.

use std::path::Path;
use std::process::Command;

use crate::backup::{self, RestoreSource, SnapshotKind};
use crate::errors::AppResult;
use crate::fans;
use crate::gpu;
use crate::jobs::{Job, JobResult};
use crate::power;
use crate::profile::{self, Mode};
use crate::startup::{self, StartupAction};

/// Scheme-name prefix. Only schemes carrying this exact prefix are ever
/// deleted, so built-in schemes are structurally out of reach.
const SCHEME_PREFIX: &str = "XMG Focus - ";

pub fn run(job: Job) -> JobResult {
    match job {
        Job::EnsureFactoryBackup => ensure_factory(),
        Job::CreateFactoryBackup { note } => create_factory(&note),
        Job::Snapshot { reason, manual } => do_snapshot(&reason, manual),
        Job::Apply {
            mode,
            control_fans,
            control_gpu,
        } => do_apply(mode, control_fans, control_gpu),
        Job::Restore { source } => do_restore(&source),
    }
}

// ------------------------------------------------------------- snapshots ---

fn do_snapshot(reason: &str, manual: bool) -> JobResult {
    let kind = if manual {
        SnapshotKind::Manual
    } else {
        SnapshotKind::Automatic
    };
    match backup::snapshot(kind, reason) {
        Ok(s) => JobResult {
            ok: true,
            summary: format!("Snapshot saved ({})", s.created),
            details: vec![s.path],
            warnings: vec![],
        },
        Err(e) => JobResult::fail(format!("Snapshot failed: {e}")),
    }
}

fn ensure_factory() -> JobResult {
    if backup::has_factory_backup() {
        return JobResult {
            ok: true,
            summary: "Initial backup already present - kept untouched".into(),
            details: vec![],
            warnings: vec![],
        };
    }
    create_factory("first run")
}

fn create_factory(note: &str) -> JobResult {
    let base = crate::paths::backup_dir().ok();
    let was_archived = base
        .as_deref()
        .map(backup::factory_is_archived)
        .unwrap_or(false);

    match backup::create_factory_backup(note) {
        Ok(a) => {
            let mut details = vec![format!("Saved to {}", a.path)];
            if was_archived {
                details.push("Previous initial backup was archived, not overwritten.".into());
            }
            JobResult {
                ok: true,
                summary: format!("New initial backup created ({})", a.id),
                details,
                warnings: vec![],
            }
        }
        Err(e) => JobResult::fail(format!("Could not create initial backup: {e}")),
    }
}

// ----------------------------------------------------------------- apply ---

fn do_apply(mode: Mode, control_fans: bool, control_gpu: bool) -> JobResult {
    let mut r = JobResult {
        ok: true,
        summary: String::new(),
        details: vec![],
        warnings: vec![],
    };

    // A snapshot is always taken *before* anything changes - including on a
    // no-op re-apply, so there is always a record of the state at that moment.
    match backup::snapshot(SnapshotKind::Automatic, &format!("before-{}", mode.label().to_lowercase())) {
        Ok(s) => r.details.push(format!("Snapshot: {}", s.created)),
        Err(e) => r.warnings.push(format!("snapshot failed: {e}")),
    }

    // The initial backup must exist before the first change.
    if !backup::has_factory_backup() {
        match backup::create_factory_backup("auto, before first apply") {
            Ok(a) => r.details.push(format!("Initial backup created: {}", a.id)),
            Err(e) => r.warnings.push(format!("initial backup failed: {e}")),
        }
    } else {
        r.details.push("Initial backup present - preserved".into());
    }

    let on_ac = power::on_ac_power();
    let (params, entries) = profile::plan(mode, on_ac);

    // Idempotence: if the active scheme is already ours for this mode and every
    // setting matches, leave it completely alone. This is what makes the
    // start-up re-apply a genuine no-op rather than a rewrite.
    let report = crate::drift::check(mode, on_ac);
    if report.fully_applied() {
        r.details.push(format!(
            "Power settings already correct - nothing to change ({} of {} verified)",
            report.total() - report.drifted(),
            report.total()
        ));
        finish_apply(&mut r, mode, on_ac, params, control_fans, control_gpu);
        return r;
    }

    let base = match power::active_scheme() {
        Ok(g) => g,
        Err(e) => return JobResult::fail(format!("Cannot read the active power scheme: {e}")),
    };

    // Build the scheme for this mode, dropping our own previous schemes only.
    cleanup_our_schemes(&base);

    let new_guid = match power::duplicate_scheme(&base) {
        Ok(g) => g,
        Err(e) => return JobResult::fail(format!("Could not create a power scheme: {e}")),
    };
    let label = format!("{SCHEME_PREFIX}{}", mode.label());
    let desc = format!(
        "{} | {} | {}",
        mode.label(),
        if on_ac { "AC" } else { "Battery" },
        chrono::Local::now().format("%d.%m.%Y")
    );
    let _ = power::rename_scheme(&new_guid, &label, &desc);

    let mut applied = 0usize;
    let mut failed: Vec<&str> = Vec::new();
    for e in &entries {
        match power::set_value(&new_guid, e.subgroup, e.setting, e.ac, e.dc) {
            Ok(true) => applied += 1,
            _ => failed.push(e.label),
        }
    }
    let _ = power::set_active(&new_guid);

    let (verified, mismatches) = power::verify(&new_guid, &entries);
    r.details.push(format!("Power settings: {applied} written, {verified} verified"));
    if !failed.is_empty() {
        r.warnings.push(format!("could not write: {}", failed.join(", ")));
    }
    if !mismatches.is_empty() {
        r.warnings
            .extend(mismatches.into_iter().take(4).map(|m| format!("mismatch: {m}")));
    }

    finish_apply(&mut r, mode, on_ac, params, control_fans, control_gpu);
    r
}

/// The parts of an apply that always run, whether or not the power settings
/// needed rewriting: startup entries, GPU limit and fans.
fn finish_apply(
    r: &mut JobResult,
    mode: Mode,
    on_ac: bool,
    params: profile::Params,
    control_fans: bool,
    control_gpu: bool,
) {
    // Startup entries
    for (name, label) in startup::DISABLED {
        match startup::apply(StartupAction::Disable, name, 0) {
            Ok(msg) => r.details.push(format!("{label}: {msg}")),
            Err(e) => r.warnings.push(format!("{label}: {e}")),
        }
    }
    for (name, delay, label) in startup::DELAYED {
        match startup::apply(StartupAction::Delay(*delay), name, *delay) {
            Ok(msg) => r.details.push(format!("{label}: {msg}")),
            Err(e) => r.warnings.push(format!("{label}: {e}")),
        }
    }

    // GPU power limit
    if control_gpu {
        match gpu::set_limit_capped(params.gpu_watts) {
            gpu::ApplyOutcome::Applied(w) => r.details.push(format!("GPU limit: {w} W")),
            gpu::ApplyOutcome::Refused { wanted, max } => r.warnings.push(format!(
                "GPU limit {wanted} W is above the hardware maximum ({max} W) - left unchanged"
            )),
            gpu::ApplyOutcome::Failed => r.warnings.push("GPU limit could not be set".into()),
            gpu::ApplyOutcome::Unavailable => {
                r.warnings.push("nvidia-smi not available - GPU limit untouched".into())
            }
        }
    }

    // Fans
    if control_fans {
        match fans::apply(&params) {
            fans::FanOutcome::Applied(m) => r.details.push(format!("Fans: {m}")),
            fans::FanOutcome::Unavailable(m) => r.warnings.push(format!("Fans: {m}")),
            fans::FanOutcome::Failed(m) => r.warnings.push(format!("Fans: {m}")),
        }
    }

    r.details.push("Game DVR off, Game Mode on".into());

    let _ = std::fs::write(
        crate::paths::app_data_dir().join("active-mode.txt"),
        mode.label(),
    );

    r.summary = format!(
        "{} applied on {}",
        mode.label(),
        if on_ac { "mains power" } else { "battery" }
    );
}

/// Deletes only power schemes this app created before.
fn cleanup_our_schemes(keep: &str) {
    for (guid, name) in power::list_schemes() {
        if name.starts_with(SCHEME_PREFIX) && guid != keep {
            let _ = power::delete_scheme(&guid);
        }
    }
}

// --------------------------------------------------------------- restore ---

fn do_restore(source: &RestoreSource) -> JobResult {
    let mut r = JobResult {
        ok: true,
        summary: String::new(),
        details: vec![],
        warnings: vec![],
    };

    // A snapshot of the *current* state first, so a restore can be undone.
    let kind_label = match source {
        RestoreSource::Snapshot(_) => "before-restore",
        _ => "before-restore-to-initial",
    };
    match backup::snapshot(SnapshotKind::Automatic, kind_label) {
        Ok(s) => r.details.push(format!("Snapshot of current state: {}", s.created)),
        Err(e) => r.warnings.push(format!("snapshot failed: {e}")),
    }

    let file = match backup::resolve_source(source) {
        Ok(p) => p,
        Err(e) => return JobResult::fail(format!("Nothing was changed. {e}")),
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => return JobResult::fail(format!("Could not read the backup: {e}")),
    };
    let rows = backup::parse_backup(&text);
    if rows.is_empty() {
        return JobResult::fail("That backup contains no usable settings. Nothing was changed.");
    }

    let scheme = match power::active_scheme() {
        Ok(g) => g,
        Err(e) => return JobResult::fail(format!("Cannot read the active power scheme: {e}")),
    };

    let rep = backup::apply_restore(&scheme, &rows);
    r.details.push(format!(
        "Power settings: {} of {} restored",
        rep.applied,
        rows.len()
    ));
    if !rep.failed.is_empty() {
        r.warnings
            .push(format!("{} could not be restored: {}", rep.failed.len(), rep.failed.join(", ")));
    }

    // Restore startup entries and the GPU limit when an initial backup is used.
    if matches!(source, RestoreSource::Factory) {
        if let Ok(base) = crate::paths::backup_dir() {
            let (ok, skipped) = startup::restore_autostart(&base);
            r.details.push(format!("Startup entries: {ok} restored"));
            if !skipped.is_empty() {
                r.warnings.push(format!("skipped: {}", skipped.join(", ")));
            }
            if let Some(w) = gpu::stored_limit(&base) {
                if let Ok(true) = gpu::set_limit(w) {
                    r.details.push(format!("GPU power limit: {w} W"));
                } else {
                    r.warnings.push("GPU power limit could not be restored".into());
                }
            }
        }
    }

    // Put the machine back on a clean, un-managed scheme so no stale profile
    // stays active behind the restored values.
    let _ = power::set_active(&scheme);
    let _ = std::fs::remove_file(crate::paths::app_data_dir().join("active-mode.txt"));

    r.summary = format!("Restored from {}", source_label(source));
    r
}

fn source_label(s: &RestoreSource) -> String {
    match s {
        RestoreSource::Factory => "the initial backup".into(),
        RestoreSource::Archive(id) => format!("archived backup {id}"),
        RestoreSource::Snapshot(id) => format!("snapshot {id}"),
    }
}

// ------------------------------------------------------------------ misc ---

/// Writes a line to the app log. Best effort; never fatal.
pub fn log(line: &str) {
    use std::io::Write;
    let p = crate::paths::log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{stamp}  {line}");
        // keep the log bounded
        if let Ok(meta) = std::fs::metadata(&p) {
            if meta.len() > 512 * 1024 {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Opens the backup folder in Explorer so the user can see their own files.
pub fn open_backup_folder() {
    if let Ok(dir) = crate::paths::backup_dir() {
        let _ = Command::new("explorer.exe").arg(dir).spawn();
    }
}

/// Path of a backup file, for "show in folder".
pub fn reveal(path: &Path) {
    let _ = Command::new("explorer.exe")
        .arg("/select,")
        .arg(path)
        .spawn();
}

/// Convenience for the worker's entry point.
pub fn execute(job: Job) -> AppResult<JobResult> {
    log(&format!("job: {:?}", std::mem::discriminant(&job)));
    Ok(run(job))
}